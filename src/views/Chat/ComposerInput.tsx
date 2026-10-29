import React, { useCallback, useEffect, useLayoutEffect, useRef } from "react";

type TokenKind = "skill" | "tool";

const slashPattern = /(?:^|\s)\/(?:tool:)?[a-z0-9:_-]+\/?(?=\s|$)/gi;
const icons: Record<TokenKind, string> = { skill: "✦", tool: "⚙" };

function escapeHtml(value: string) {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/\n/g, "<br>");
}

function collectTokens(value: string, skillIds: Set<string>, toolIds: Set<string>) {
  const tokens: Array<{ start: number; end: number; value: string; kind: TokenKind }> = [];
  for (const match of value.matchAll(slashPattern)) {
    const raw = match[0];
    const offset = raw.startsWith(" ") ? 1 : 0;
    const token = raw.slice(offset);
    const start = (match.index ?? 0) + offset;
    const directive = token.slice(1).replace(/\/$/, "");
    const toolId = directive.startsWith("tool:") ? directive.slice("tool:".length) : "";
    const known = toolId ? toolIds.has(toolId) : skillIds.has(directive);
    if (!known) continue;
    tokens.push({ start, end: start + token.length, value: token, kind: toolId ? "tool" : "skill" });
  }
  return tokens.filter((token, index, list) => index === 0 || token.start >= list[index - 1].end);
}

function readComposerText(root: HTMLElement) {
  const readNode = (node: Node): string => {
    if (node instanceof HTMLElement && node.classList.contains("composer-token")) return node.dataset.composerValue || "";
    if (node.nodeType === Node.TEXT_NODE) return node.textContent || "";
    if (node instanceof HTMLBRElement) return "\n";
    return [...node.childNodes].map(readNode).join("");
  };
  return readNode(root).replace(/\u00a0/g, " ").replace(/\n$/, "");
}

function placeCaretAtEnd(element: HTMLElement) {
  const selection = window.getSelection();
  if (!selection) return;
  let lastChild = element.lastChild;
  if (lastChild instanceof HTMLElement && lastChild.classList.contains("composer-token")) lastChild = element.appendChild(document.createTextNode(""));
  const range = document.createRange();
  if (lastChild?.nodeType === Node.TEXT_NODE) range.setStart(lastChild, lastChild.textContent?.length || 0);
  else range.selectNodeContents(element);
  range.collapse(lastChild?.nodeType === Node.TEXT_NODE);
  selection.removeAllRanges();
  selection.addRange(range);
}

function syncComposerHeight(element: HTMLElement) {
  // Reset first so deleting or wrapping text can shrink the editor again.
  const style = getComputedStyle(element);
  const parsedMin = parseFloat(style.minHeight);
  const parsedMax = parseFloat(style.maxHeight);
  // Defensive fallback: a missing/abnormal computed style would make the clamp
  // return NaN and leave the inline height effectively unbounded.
  const minHeight = Number.isFinite(parsedMin) ? parsedMin : 46;
  const maxHeight = Number.isFinite(parsedMax) ? parsedMax : 220;
  const scrollTop = element.scrollTop;
  element.style.height = "auto";
  const contentHeight = element.scrollHeight;
  const height = Math.min(Math.max(contentHeight, minHeight), maxHeight);
  element.style.height = `${height}px`;
  element.style.overflowY = contentHeight > maxHeight ? "auto" : "hidden";
  element.scrollTop = scrollTop;
}

function revealComposerCaret(element: HTMLElement) {
  if (document.activeElement !== element) return;
  const selection = window.getSelection();
  if (!selection?.rangeCount || !selection.isCollapsed) return;
  const caret = selection.getRangeAt(0);
  if (!element.contains(caret.startContainer)) return;
  const contents = document.createRange();
  contents.selectNodeContents(element);
  if (caret.compareBoundaryPoints(Range.END_TO_END, contents) === 0) {
    element.scrollTop = element.scrollHeight;
    return;
  }
  const rect = caret.getBoundingClientRect();
  if (!rect.height) return;
  const bounds = element.getBoundingClientRect();
  const top = bounds.top + element.clientTop + 4;
  const bottom = bounds.top + element.clientTop + element.clientHeight - 4;
  if (rect.top < top) element.scrollTop -= top - rect.top;
  else if (rect.bottom > bottom) element.scrollTop += rect.bottom - bottom;
}

export function ComposerInput({
  value,
  skillIds,
  toolIds,
  onChange,
  onKeyDown,
  onPaste,
  onEditorReady,
}: {
  value: string;
  skillIds: string[];
  toolIds: string[];
  onChange: (value: string) => void;
  onKeyDown: (event: React.KeyboardEvent<HTMLDivElement>) => void;
  onPaste: (event: React.ClipboardEvent<HTMLDivElement>) => void;
  onEditorReady?: (element: HTMLDivElement | null) => void;
}) {
  const ref = useRef<HTMLDivElement | null>(null);
  const composing = useRef(false);
  const rendered = useRef("");
  const setEditorRef = useCallback((element: HTMLDivElement | null) => {
    ref.current = element;
    onEditorReady?.(element);
  }, [onEditorReady]);

  const render = (next: string) => {
    const element = ref.current;
    if (!element) return;
    const tokens = collectTokens(next, new Set(skillIds), new Set(toolIds));
    const signature = tokens.map((token) => `${token.start}:${token.end}:${token.kind}:${token.value}`).join("|");
    if (element.dataset.composerTokenSignature === signature && readComposerText(element) === next) {
      rendered.current = next;
      syncComposerHeight(element);
      revealComposerCaret(element);
      return;
    }
    let html = "";
    let cursor = 0;
    for (const token of tokens) {
      html += escapeHtml(next.slice(cursor, token.start));
      html += `<span class="composer-token composer-${token.kind}-token" contenteditable="false" data-composer-value="${escapeHtml(token.value)}"><span class="composer-token-icon">${icons[token.kind]}</span>${escapeHtml(token.value)}</span>`;
      cursor = token.end;
    }
    html += escapeHtml(next.slice(cursor));
    element.innerHTML = html || "";
    element.dataset.composerTokenSignature = signature;
    rendered.current = next;
    placeCaretAtEnd(element);
    syncComposerHeight(element);
    revealComposerCaret(element);
  };

  useLayoutEffect(() => {
    if (composing.current || !ref.current) return;
    if (readComposerText(ref.current) !== value) render(value);
  });

  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    const editors = document.querySelectorAll(".composer-input");
    if (editors.length > 1) console.warn(`[composer] duplicate editor nodes detected: ${editors.length}`);
    let width = 0;
    const observer = new ResizeObserver(([entry]) => {
      if (entry.contentRect.width === width) return;
      width = entry.contentRect.width;
      syncComposerHeight(element);
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  return <div
    ref={setEditorRef}
    className="composer-input"
    contentEditable
    role="textbox"
    aria-multiline="true"
    data-placeholder="描述问题…"
    suppressContentEditableWarning
    onCompositionStart={() => { composing.current = true; }}
    onCompositionEnd={() => { composing.current = false; onChange(readComposerText(ref.current!)); syncComposerHeight(ref.current!); revealComposerCaret(ref.current!); }}
    onInput={() => {
      const next = readComposerText(ref.current!);
      onChange(next);
      if (!composing.current) render(next);
      else { syncComposerHeight(ref.current!); revealComposerCaret(ref.current!); }
    }}
    onPaste={onPaste}
    onKeyDown={(event) => {
      if ((event.key === "Backspace" || event.key === "Delete") && ref.current) {
        const selection = window.getSelection();
        const range = selection?.rangeCount ? selection.getRangeAt(0) : null;
        if (range && selection?.isCollapsed && ref.current.contains(range.startContainer)) {
          const parent = range.startContainer.nodeType === Node.TEXT_NODE ? range.startContainer.parentNode : range.startContainer;
          const nodes = parent ? [...parent.childNodes] : [];
          const index = range.startContainer.nodeType === Node.TEXT_NODE ? nodes.indexOf(range.startContainer as ChildNode) : range.startOffset;
          const neighbor = event.key === "Backspace" ? nodes[index - 1] : nodes[index];
          const atEdge = range.startContainer.nodeType !== Node.TEXT_NODE || (event.key === "Backspace" ? range.startOffset === 0 : range.startOffset === (range.startContainer.textContent?.length || 0));
          if (atEdge && neighbor instanceof HTMLElement && neighbor.classList.contains("composer-token")) {
            event.preventDefault();
            const token = neighbor.dataset.composerValue || "";
            neighbor.remove();
            const text = readComposerText(ref.current).replace(token, "");
            onChange(text);
            render(text);
            return;
          }
        }
      }
      onKeyDown(event);
    }}
  />;
}
