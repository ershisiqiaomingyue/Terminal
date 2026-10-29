import DOMPurify from "dompurify";
import React, { memo, useMemo, useRef } from "react";
import { Check, Copy } from "lucide-react";
import { renderToStaticMarkup } from "react-dom/server";
import { markdownToUnsafeHtml, normalizeMarkdownStructure } from "./markdownProjection";
import { IncrementalMarkdown } from "./incrementalMarkdown";
import { useLongMarkdown } from "./useLongMarkdown";
import { BoundedTextCache } from "./markdownCache";
import { localPathMatches } from "./localPathMatches";
import type { LongMarkdownBlock } from "./longMarkdownProjection";
import { attachDiagramHost } from "@/components/MarkdownDiagram/diagramSource";
import { useDiagramPortals } from "@/components/MarkdownDiagram/useDiagramPortals";

export { normalizeMarkdownStructure };

const CODE_COPY_ICONS = renderToStaticMarkup(<><Copy size={13} className="code-copy-icon" aria-hidden="true" /><Check size={13} className="code-check-icon" aria-hidden="true" /></>);
const copyResetTimers = new WeakMap<HTMLButtonElement, number>();
const renderedMarkdown = new BoundedTextCache<string>(128, 2_000_000);

function decorateLocalPaths(documentRef: Document) {
  const root = documentRef.body.firstElementChild;
  if (!root) return;
  const walker = documentRef.createTreeWalker(root, 4 /* NodeFilter.SHOW_TEXT */);
  const textNodes: Text[] = [];
  while (walker.nextNode()) {
    const node = walker.currentNode as Text;
    const parent = node.parentElement;
    if (!parent || parent.closest("a, code, pre, .message-local-path")) continue;
    if (localPathMatches(node.data).length) textNodes.push(node);
  }
  for (const node of textNodes) {
    const fragment = documentRef.createDocumentFragment();
    let offset = 0;
    for (const match of localPathMatches(node.data)) {
      const { index, path, trailing } = match;
      if (index > offset) fragment.append(node.data.slice(offset, index));
      const span = documentRef.createElement("span");
      span.className = "message-local-path";
      span.dataset.localPath = path;
      span.textContent = path;
      fragment.append(span);
      if (trailing) fragment.append(trailing);
      offset = match.end;
    }
    if (offset < node.data.length) fragment.append(node.data.slice(offset));
    node.replaceWith(fragment);
  }
}

function renderMarkdown(source: string, streaming: boolean) {
  const key = `markdown:${source}`;
  if (!streaming) {
    const cached = renderedMarkdown.get(key);
    if (cached !== undefined) return cached;
  }
  const parsed = markdownToUnsafeHtml(source, { streaming });
  const html = decorateMarkdownHtml(parsed);
  if (!streaming) renderedMarkdown.set(key, html, html.length);
  return html;
}

function decorateMarkdownHtml(parsed: string) {
  if (!parsed) return "";
  const clean = DOMPurify.sanitize(parsed, {
    USE_PROFILES: { html: true },
    FORBID_TAGS: ["style", "script", "iframe", "object", "embed"],
    ALLOWED_URI_REGEXP: /^(?:(?:https?|mailto|tel):|[./#?]|[^:]*$)/i,
  });
  const documentRef = new DOMParser().parseFromString(`<div>${clean}</div>`, "text/html");
  decorateLocalPaths(documentRef);
  for (const pre of Array.from(documentRef.querySelectorAll("pre"))) {
    if (attachDiagramHost(pre)) continue;
    const wrapper = documentRef.createElement("div");
    wrapper.className = "markdown-code-block";
    const button = documentRef.createElement("button");
    button.type = "button";
    button.className = "code-copy-button";
    button.dataset.copyCode = "true";
    button.dataset.copied = "false";
    button.setAttribute("aria-label", "复制代码");
    button.setAttribute("title", "复制代码");
    button.innerHTML = CODE_COPY_ICONS;
    pre.replaceWith(wrapper);
    wrapper.append(pre, button);
  }
  for (const table of Array.from(documentRef.querySelectorAll("table"))) {
    const wrapper = documentRef.createElement("div");
    wrapper.className = "markdown-table-container";
    table.replaceWith(wrapper);
    wrapper.append(table);
    const headerCells = Array.from(table.querySelectorAll("thead th"));
    if (headerCells.length < 2) continue;
    const colgroup = documentRef.createElement("colgroup");
    for (let index = 0; index < headerCells.length; index += 1) {
      const col = documentRef.createElement("col");
      col.style.width = `${100 / headerCells.length}%`;
      colgroup.append(col);
      if (index < headerCells.length - 1) {
        const handle = documentRef.createElement("span");
        handle.className = "markdown-table-resize";
        handle.dataset.tableResize = String(index);
        handle.setAttribute("role", "separator");
        handle.setAttribute("aria-label", "调整表格列宽");
        handle.setAttribute("aria-orientation", "vertical");
        handle.setAttribute("title", "拖动调整列宽");
        headerCells[index].append(handle);
      }
    }
    table.prepend(colgroup);
    table.style.tableLayout = "fixed";
  }
  return documentRef.body.firstElementChild?.innerHTML || clean;
}

const MarkdownPartView = memo(function MarkdownPartView({ source, streaming }: { source: string; streaming: boolean }) {
  const html = useMemo(() => renderMarkdown(source, streaming), [source, streaming]);
  return <div dangerouslySetInnerHTML={{ __html: html }} />;
});

const MarkdownHtmlPart = memo(function MarkdownHtmlPart({ html, inline = false }: { html: string; inline?: boolean }) {
  const clean = useMemo(() => {
    const key = `html:${html}`;
    const cached = renderedMarkdown.get(key);
    if (cached !== undefined) return cached;
    const result = decorateMarkdownHtml(html);
    renderedMarkdown.set(key, result, result.length);
    return result;
  }, [html]);
  return inline ? <span className="markdown-inline-chunk" dangerouslySetInnerHTML={{ __html: clean }} /> : <div dangerouslySetInnerHTML={{ __html: clean }} />;
});

function LongMarkdownView({ blocks }: { blocks: LongMarkdownBlock[] }) {
  return <>{blocks.map((block, index) => block.kind === "paragraph"
    ? <div key={index}><p>{block.chunks.map((html, chunk) => <MarkdownHtmlPart key={chunk} html={html} inline />)}</p></div>
    : <MarkdownHtmlPart key={index} html={block.html} />)}</>;
}

const LongMarkdownPlainText = memo(function LongMarkdownPlainText({ source }: { source: string }) {
  const chunks = useMemo(() => {
    const result: string[] = [];
    for (let start = 0; start < source.length;) {
      let end = Math.min(start + 4096, source.length);
      if (end < source.length && /[\uD800-\uDBFF]/.test(source[end - 1])) end -= 1;
      result.push(source.slice(start, end));
      start = end;
    }
    return result;
  }, [source]);
  return <div className="markdown-plain-fallback">{chunks.map((chunk, index) => <span key={index}>{chunk}</span>)}</div>;
});

function MarkdownMessageComponent({ content, streaming, onPathMenu }: { content: string; streaming?: boolean; onPathMenu?: (path: string, x: number, y: number) => void }) {
  const element = useRef<HTMLDivElement>(null);
  const diagrams = useDiagramPortals(element, Boolean(streaming));
  const long = content.length >= 16384;
  const background = useLongMarkdown(content, Boolean(streaming), long);
  const parser = useMemo(() => new IncrementalMarkdown(), [streaming]);
  const parts = useMemo(() => {
    // Keep a bounded preview while the first background parse is pending.
    const source = long ? content.slice(0, 16384) : content;
    return streaming ? parser.update(normalizeMarkdownStructure(source)) : [{ offset: 0, source, streaming: false }];
  }, [content, parser, streaming, long]);
  const onClick = async (event: React.MouseEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement;
    const button = target.closest<HTMLButtonElement>("[data-copy-code]");
    if (!button) return;
    const code = button.parentElement?.querySelector("pre code")?.textContent || "";
    const previousTimer = copyResetTimers.get(button);
    if (previousTimer !== undefined) window.clearTimeout(previousTimer);
    const feedback = (copied: boolean, label: string) => {
      button.dataset.copied = String(copied);
      button.title = label;
      button.setAttribute("aria-label", label);
      copyResetTimers.set(button, window.setTimeout(() => {
        button.dataset.copied = "false";
        button.title = "复制代码";
        button.setAttribute("aria-label", "复制代码");
        copyResetTimers.delete(button);
      }, 1600));
    };
    try {
      await navigator.clipboard.writeText(code);
      feedback(true, "已复制");
    } catch {
      feedback(false, "复制失败");
    }
  };
  const onMouseDown = (event: React.MouseEvent<HTMLDivElement>) => {
    const handle = (event.target as HTMLElement).closest<HTMLElement>("[data-table-resize]");
    if (!handle) return;
    const table = handle.closest("table");
    const index = Number(handle.dataset.tableResize);
    const columns = table?.querySelectorAll<HTMLTableColElement>("colgroup col");
    if (!table || !columns || !Number.isInteger(index) || !columns[index + 1]) return;
    event.preventDefault();
    const startX = event.clientX;
    const width = table.getBoundingClientRect().width;
    const firstWidth = columns[index].getBoundingClientRect().width;
    const secondWidth = columns[index + 1].getBoundingClientRect().width;
    const move = (moveEvent: MouseEvent) => {
      const delta = moveEvent.clientX - startX;
      const nextFirst = Math.max(56, Math.min(firstWidth + secondWidth - 56, firstWidth + delta));
      columns[index].style.width = `${(nextFirst / width) * 100}%`;
      columns[index + 1].style.width = `${((firstWidth + secondWidth - nextFirst) / width) * 100}%`;
    };
    const up = () => { window.removeEventListener("mousemove", move); window.removeEventListener("mouseup", up); };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };
  if (!content.trim()) return null;
  return <div ref={element} className={`markdown-body${streaming ? " streaming" : ""}`} onClick={(event) => {
    const path = (event.target as HTMLElement).closest<HTMLElement>(".message-local-path")?.dataset.localPath;
    if (path && event.ctrlKey) { event.preventDefault(); onPathMenu?.(path, event.clientX, event.clientY); }
    onClick(event);
  }} onContextMenu={(event) => {
    const path = (event.target as HTMLElement).closest<HTMLElement>(".message-local-path")?.dataset.localPath;
    if (!path || !onPathMenu) return;
    event.preventDefault();
    onPathMenu(path, event.clientX, event.clientY);
  }} onMouseDown={onMouseDown}>
    {long && background.failed
      ? <LongMarkdownPlainText source={content} />
      : long && background.result
      ? <LongMarkdownView blocks={background.result.blocks} />
      : parts.map((part) => <MarkdownPartView key={part.offset} source={part.source} streaming={part.streaming} />)}
    {diagrams}
  </div>;
}

export const MarkdownMessage = memo(MarkdownMessageComponent);

/** Kept separately so message toolbars can use the same copy affordance. */
export function CodeCopyButton({ onCopy, copied }: { onCopy: () => void; copied: boolean }) {
  return <button type="button" className="code-copy-button" data-copy-code data-copied={copied} onClick={onCopy} title="复制代码">{copied ? <Check size={13} /> : <Copy size={13} />}</button>;
}
