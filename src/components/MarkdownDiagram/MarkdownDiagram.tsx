import { memo, useEffect, useMemo, useRef, useState } from "react";
import { Check, Copy, Download, Maximize2, Minus, Plus, RotateCcw, X } from "lucide-react";
import { subscribeAppTheme } from "@/platform/theme";
import { subscribeAppFont } from "@/platform/font";
import { formatInvokeError, invoke, isTauri } from "@/platform/runtime";
import { notify } from "@/platform/notifications";
import { reportDiagnostic } from "@/platform/frontendHealth";
import { getCachedDiagram, isolateDiagramIds, readDiagramPalette, renderDiagram } from "./diagramRuntime";
import { diagramSourceError } from "./diagramSource";
import "./markdown-diagram.css";

export const MarkdownDiagram = memo(function MarkdownDiagram({ source, streaming = false }: { source: string; streaming?: boolean }) {
  const element = useRef<HTMLElement>(null);
  const dialog = useRef<HTMLDialogElement>(null);
  const opener = useRef<HTMLButtonElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout>>();
  const downloadInProgress = useRef(false);
  const [theme, setTheme] = useState(0);
  const [result, setResult] = useState(() => {
    const cached = !streaming && typeof document !== "undefined" ? getCachedDiagram(source, readDiagramPalette(document.documentElement)) : undefined;
    return { source: cached ? source : "", theme: cached ? 0 : -1, svg: cached ? isolateDiagramIds(cached) : "", error: "" };
  });
  const [view, setView] = useState<"diagram" | "code">("diagram");
  const [copy, setCopy] = useState("");
  const [zoom, setZoom] = useState(1);
  const [downloading, setDownloading] = useState(false);
  const ready = !streaming && result.source === source && result.theme === theme;
  const svg = !streaming && result.source === source ? result.svg : "";
  const enlargedSvg = useMemo(() => isolateDiagramIds(svg), [svg]);
  const error = ready ? result.error : "";
  useEffect(() => {
    let palette = JSON.stringify(readDiagramPalette(element.current!));
    /* 调色板里带着 --font-sans：界面字体一换，已渲染的图还是旧字体。
       字体事件和主题事件走同一条「变了就重渲染」的路径。 */
    const sync = () => {
      const next = JSON.stringify(readDiagramPalette(element.current!));
      if (next !== palette) { palette = next; setTheme((value) => value + 1); }
    };
    const unsubscribeTheme = subscribeAppTheme(sync);
    const unsubscribeFont = subscribeAppFont(sync);
    return () => { unsubscribeTheme(); unsubscribeFont(); };
  }, []);
  useEffect(() => {
    if (streaming || !element.current) return;
    if (result.source === source && result.theme === theme && result.svg) return;
    let active = true;
    void renderDiagram(source, readDiagramPalette(element.current), () => active).then((svg) => {
      if (active) setResult({ source, theme, svg, error: "" });
    }, () => {
      reportDiagnostic("diagram_render_failed", { sourceLength: source.length }, "warn");
      if (active) setResult({ source, theme, svg: "", error: diagramSourceError(source) || "图表无法渲染，请检查语法。已保留源代码。" });
    });
    return () => { active = false; };
  }, [source, streaming, theme]);
  useEffect(() => () => { if (timer.current) clearTimeout(timer.current); }, []);
  const copySource = async () => {
    try { await navigator.clipboard.writeText(source); setCopy("已复制"); }
    catch { setCopy("复制失败"); }
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => setCopy(""), 1500);
  };
  const exportSvg = async () => {
    if (!svg || downloadInProgress.current) return;
    downloadInProgress.current = true;
    setDownloading(true);
    try {
      if (isTauri()) {
        const path = await invoke<string>("download_mermaid_svg", { svg });
        notify(`图表已保存：${path}`, "success");
      } else {
        const url = URL.createObjectURL(new Blob([svg], { type: "image/svg+xml;charset=utf-8" }));
        const link = document.createElement("a");
        link.href = url; link.download = "mermaid.svg";
        document.body.append(link); link.click(); link.remove();
        setTimeout(() => URL.revokeObjectURL(url), 1000);
        notify("已开始下载图表", "success");
      }
    } catch (error) { notify(`图表下载失败：${formatInvokeError(error)}`, "error"); }
    finally { downloadInProgress.current = false; setDownloading(false); }
  };
  return <section ref={element} className="markdown-diagram" aria-label="Mermaid 图表" data-state={streaming ? "streaming" : !ready ? "loading" : error ? "error" : "ready"}>
    <div className="markdown-diagram-toolbar" data-file-search-ignore>
      <span>Mermaid</span>
      <div className="markdown-diagram-views" role="group" aria-label="图表视图">
        <button type="button" aria-pressed={view === "diagram"} onClick={() => setView("diagram")}>图形</button>
        <button type="button" aria-pressed={view === "code"} onClick={() => setView("code")}>代码</button>
      </div>
      <button type="button" title={copy || "复制图源"} aria-label={copy || "复制图源"} onClick={() => void copySource()}>{copy === "已复制" ? <Check size={14} /> : <Copy size={14} />}</button>
      <button type="button" title={downloading ? "正在下载图表…" : "下载 SVG 到下载目录"} aria-label="下载 SVG 到下载目录" aria-busy={downloading} disabled={!svg || downloading} onClick={() => void exportSvg()}><Download size={14} /></button>
      <button ref={opener} type="button" title="放大图表" aria-label="放大图表" disabled={!svg} onClick={() => { setZoom(1); dialog.current?.showModal(); }}><Maximize2 size={14} /></button>
    </div>
    {error && <p className="markdown-diagram-status error" role="status">{error}</p>}
    {!ready && !svg && <p className="markdown-diagram-status" role="status">{streaming ? "图表生成中…" : "正在渲染图表…"}</p>}
    {view === "code" || !svg ? <pre className="markdown-diagram-code"><code>{source}</code></pre> : <div className="markdown-diagram-canvas" role="img" aria-label="Mermaid 图形" dangerouslySetInnerHTML={{ __html: svg }} />}
    <dialog ref={dialog} className="markdown-diagram-dialog" aria-label="查看图表" onClose={() => opener.current?.focus()}>
      <div className="markdown-diagram-toolbar">
        <span>Mermaid</span>
        <button type="button" title="缩小图表" aria-label="缩小图表" onClick={() => setZoom((value) => Math.max(.25, value / 1.25))}><Minus size={16} /></button>
        <button type="button" title="恢复图表大小" aria-label="恢复图表大小" onClick={() => setZoom(1)}><RotateCcw size={14} />{Math.round(zoom * 100)}%</button>
        <button type="button" title="放大图表内容" aria-label="放大图表内容" onClick={() => setZoom((value) => Math.min(4, value * 1.25))}><Plus size={16} /></button>
        <button type="button" title="关闭图表" aria-label="关闭图表" onClick={() => dialog.current?.close()}><X size={16} /></button>
      </div>
      <div className="markdown-diagram-viewport"><div className="markdown-diagram-enlarged" style={{ width: `${zoom * 100}%`, minWidth: `${zoom * 100}%` }} dangerouslySetInnerHTML={{ __html: enlargedSvg }} /></div>
    </dialog>
  </section>;
});
