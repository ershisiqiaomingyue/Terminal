import { useEffect, useMemo, useRef, useState } from "react";
import { canPreviewLongMarkdown, type LongMarkdownRequest, type LongMarkdownResult } from "./longMarkdownProjection";
import { BoundedTextCache } from "./markdownCache";
import { MarkdownWorkerQueue } from "./markdownWorkerQueue";
import { reportDiagnostic } from "@/platform/frontendHealth";

const completedMarkdown = new BoundedTextCache<LongMarkdownResult>(32, 2_000_000);
const parsers = new MarkdownWorkerQueue(() => new Worker(new URL("./markdown.worker.ts", import.meta.url), { type: "module" }));

export function useLongMarkdown(source: string, streaming: boolean, enabled: boolean) {
  const [result, setResult] = useState<LongMarkdownResult | null>(null);
  const [failed, setFailed] = useState(false);
  const queue = useRef<ReturnType<typeof parsers.subscribe>>();
  const latest = useRef<LongMarkdownRequest>({ source, streaming });
  latest.current = { source, streaming };
  const cached = useMemo(() => enabled && !streaming ? completedMarkdown.get(source) : undefined, [enabled, source, streaming]);

  useEffect(() => {
    if (!enabled || failed || cached) return;
    const client = parsers.subscribe((incoming) => {
      if (!incoming.streaming) {
        const characters = incoming.blocks.reduce((total, block) => total + (block.kind === "html" ? block.html.length : block.chunks.reduce((sum, chunk) => sum + chunk.length, 0)), 0);
        completedMarkdown.set(incoming.source, incoming, characters);
      }
      if (canPreviewLongMarkdown(incoming, latest.current)) setResult(incoming);
    }, () => {
      reportDiagnostic("markdown_parser_failed", { sourceLength: latest.current.source.length, streaming: latest.current.streaming }, "warn");
      setFailed(true);
    });
    queue.current = client;
    client.submit(latest.current);
    return () => { queue.current = undefined; client.dispose(); };
  }, [enabled, failed, Boolean(cached)]);

  useEffect(() => {
    if (enabled && !failed && !cached) queue.current?.submit({ source, streaming });
  }, [source, streaming, enabled, failed, cached]);

  const current = cached || (result && canPreviewLongMarkdown(result, { source, streaming }) ? result : null);
  return { result: failed ? null : current, failed };
}
