import DOMPurify from "dompurify";
import { marked } from "marked";
import { Check, Copy } from "lucide-react";
import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Button } from "@/components/ui";
import { PaneResizeHandle } from "./PaneResizeHandle";
import { OUTLINE_PANE } from "./paneWidth";
import { usePaneWidth } from "./usePaneWidth";
import { attachDiagramHost } from "@/components/MarkdownDiagram/diagramSource";
import { useDiagramPortals } from "@/components/MarkdownDiagram/useDiagramPortals";

const MAX_RENDER_LENGTH = 256 * 1024;

export const MarkdownFilePreview = memo(function MarkdownFilePreview({ source, showOutline, outlineId = "code-viewer-document-outline" }: { source: string; showOutline: boolean; outlineId?: string }) {
  const documentElement = useRef<HTMLDivElement>(null);
  const diagrams = useDiagramPortals(documentElement);
  const layoutElement = useRef<HTMLDivElement>(null);
  const outlinePane = usePaneWidth(layoutElement, true, "android-tool.code-viewer.outline-width", OUTLINE_PANE);
  const [copyHosts, setCopyHosts] = useState<HTMLElement[]>([]);
  const [copyState, setCopyState] = useState<{ index: number; status: "copied" | "failed" } | null>(null);
  const resetTimer = useRef<ReturnType<typeof setTimeout>>();
  const large = source.length > MAX_RENDER_LENGTH;
  const { html, headings } = useMemo(() => {
    if (large) return { html: "", headings: [] };
    const clean = DOMPurify.sanitize(marked.parse(source, { async: false, gfm: true }), {
      USE_PROFILES: { html: true },
      FORBID_TAGS: ["style", "script", "iframe", "object", "embed", "form"],
      FORBID_ATTR: ["style"],
    });
    const documentRef = new DOMParser().parseFromString(clean, "text/html");
    for (const link of documentRef.querySelectorAll("a[href]")) {
      link.setAttribute("target", "_blank");
      link.setAttribute("rel", "noopener noreferrer");
    }
    const headings = Array.from(documentRef.querySelectorAll("h1,h2,h3,h4,h5,h6"), (heading, index) => {
      const id = `file-heading-${index}`;
      heading.id = id;
      return { id, label: heading.textContent || "", level: Number(heading.tagName.slice(1)) };
    });
    for (const pre of Array.from(documentRef.querySelectorAll("pre"))) {
      if (attachDiagramHost(pre)) continue;
      const wrapper = documentRef.createElement("div");
      wrapper.className = "code-viewer-markdown-code";
      const host = documentRef.createElement("div");
      host.className = "code-viewer-code-copy";
      host.dataset.fileSearchIgnore = "true";
      pre.replaceWith(wrapper);
      wrapper.append(host, pre);
    }
    return { html: documentRef.body.innerHTML, headings };
  }, [source, large]);

  useLayoutEffect(() => {
    setCopyHosts(Array.from(documentElement.current?.querySelectorAll<HTMLElement>(".code-viewer-code-copy") || []));
    setCopyState(null);
    if (resetTimer.current) clearTimeout(resetTimer.current);
  }, [html]);
  useEffect(() => () => { if (resetTimer.current) clearTimeout(resetTimer.current); }, []);
  const copyCode = async (host: HTMLElement, index: number) => {
    const code = host.parentElement?.querySelector("pre")?.textContent || "";
    try { await navigator.clipboard.writeText(code); if (!host.isConnected) return; setCopyState({ index, status: "copied" }); }
    catch { if (!host.isConnected) return; setCopyState({ index, status: "failed" }); }
    if (resetTimer.current) clearTimeout(resetTimer.current);
    resetTimer.current = setTimeout(() => setCopyState(null), 1500);
  };

  return <div ref={layoutElement} className={`code-viewer-markdown-layout${showOutline && headings.length ? " has-outline" : ""}`} style={{ "--outline-pane-width": `${outlinePane.width}px` } as React.CSSProperties}>
    {showOutline && headings.length > 0 && <nav id={outlineId} className="code-viewer-outline custom-scrollbar" aria-label="文档目录">
      <span>目录</span>
      {headings.map((heading) => <button type="button" key={heading.id} style={{ paddingLeft: 8 + (heading.level - 1) * 10 }} onClick={() => documentElement.current?.querySelector(`#${heading.id}`)?.scrollIntoView({ block: "start" })}>{heading.label}</button>)}
    </nav>}
    {showOutline && headings.length > 0 && <PaneResizeHandle label="调整文档目录宽度" controls={outlineId} width={outlinePane.width} min={outlinePane.bounds.min} max={outlinePane.bounds.max} initial={OUTLINE_PANE.initial} onResize={outlinePane.resize} onCommit={outlinePane.commit} />}
    <div ref={documentElement} className="code-viewer-markdown custom-scrollbar" aria-label="Markdown 预览">
      {large ? <><p className="code-viewer-markdown-note">文件较大，显示完整原文以保持响应速度。</p><pre data-file-search-root>{source}</pre></>
        : <div data-file-search-root dangerouslySetInnerHTML={{ __html: html }} />}
    </div>
    {diagrams}
    {copyHosts.map((host, index) => createPortal(<Button type="button" variant="subtle" size="xs" icon={copyState?.index === index && copyState.status === "copied" ? <Check size={13} /> : <Copy size={13} />} title={copyState?.index === index ? copyState.status === "copied" ? "已复制" : "复制失败" : "复制代码"} aria-label={copyState?.index === index ? copyState.status === "copied" ? "已复制" : "复制失败" : "复制代码"} onClick={() => void copyCode(host, index)} />, host, String(index)))}
  </div>;
});
