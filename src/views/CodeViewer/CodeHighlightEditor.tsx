import { useCallback, useEffect, useLayoutEffect, useMemo, useRef } from "react";
import { highlightSource, MAX_HIGHLIGHT_BYTES, resolveLanguage } from "./sourceHighlight";

/** 行号槽超过这个行数就隐藏：再大的文件生成/同步行号的代价开始大于收益。 */
const GUTTER_MAX_LINES = 50_000;

// 可编辑 + 语法高亮：底层是不可编辑的高亮 <pre>，上面盖一个文字透明的
// <textarea>（只显示光标和选区）。两层必须用完全相同的字体、内边距和换行
// 规则，否则光标会和字符错位。
export function CodeHighlightEditor({ value, path, onChange, readOnly, ariaLabel, onEditor }: {
  value: string;
  path: string;
  onChange: (value: string) => void;
  readOnly?: boolean;
  ariaLabel: string;
  onEditor?: (element: HTMLTextAreaElement | null) => void;
}) {
  const editor = useRef<HTMLTextAreaElement>(null);
  const highlight = useRef<HTMLPreElement>(null);
  const gutter = useRef<HTMLDivElement>(null);
  const tooLarge = value.length > MAX_HIGHLIGHT_BYTES;
  // 认不出扩展名时给一句说明，省得用户以为是渲染坏了。
  const unsupported = !resolveLanguage(path);
  const html = tooLarge ? "" : highlightSource(value, path);
  // 行号列（用户要求：左侧行号 + 行号后面一条竖着的细线）。与正文字体/行高
  // 完全一致（12px / 1.7）、顶部内边距同 14px，行行对齐；滚动时整体平移跟随。
  const gutterNumbers = useMemo(() => {
    const count = value.split("\n").length;
    if (count > GUTTER_MAX_LINES) return null;
    const parts: string[] = [];
    for (let index = 1; index <= count; index += 1) parts.push(String(index));
    return parts.join("\n");
  }, [value]);

  // 滚动同步：textarea 是滚动源，高亮层与行号列跟着走。
  const syncScroll = useCallback(() => {
    const source = editor.current, target = highlight.current;
    if (!source || !target) return;
    target.scrollTop = source.scrollTop;
    target.scrollLeft = source.scrollLeft;
    if (gutter.current) gutter.current.style.transform = `translateY(${-source.scrollTop}px)`;
  }, []);

  useLayoutEffect(syncScroll, [value, syncScroll]);
  useEffect(() => { onEditor?.(editor.current); return () => onEditor?.(null); }, [onEditor]);

  // 高亮重新渲染后 pre 的滚动位置会被浏览器重置，这里补一次。
  useEffect(() => { syncScroll(); }, [html, syncScroll]);

  return <div className={`code-viewer-editor-stack${tooLarge ? " is-plain" : ""}${gutterNumbers ? " has-gutter" : ""}`}>
    {gutterNumbers && (
      <div className="code-viewer-gutter" aria-hidden="true">
        <div ref={gutter} className="code-viewer-gutter-inner">{gutterNumbers}</div>
      </div>
    )}
    {!tooLarge && <pre ref={highlight} className="code-viewer-editor-highlight" aria-hidden="true"><code dangerouslySetInnerHTML={{ __html: html }} /></pre>}
    <textarea ref={editor} className="code-viewer-editor code-viewer-editor-input custom-scrollbar" spellCheck={false} wrap="off"
      readOnly={readOnly} value={value} aria-label={ariaLabel}
      onChange={(event) => onChange(event.target.value)} onScroll={syncScroll} />
    {tooLarge && <small className="code-viewer-highlight-note">文件较大，已关闭语法高亮以保持响应速度。</small>}
    {!tooLarge && unsupported && <small className="code-viewer-highlight-note">这个文件类型没有语法高亮，按纯文本显示。</small>}
  </div>;
}
