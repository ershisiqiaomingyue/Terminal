/**
 * 粘贴富文本链接时的显示文本选择。
 *
 * 很多站点（GitHub 的复制按钮、Notion、掘金等）在剪贴板的 HTML 里把链接
 * 写成一个短标题，例如：
 *   <a href="https://github.com/owner/repo">owner/repo</a>
 * 而纯文本部分才是完整地址。若直接沿用短标题，粘贴后用户会以为地址被改写了，
 * 因此当标题只是地址的缩短形式时，保留完整地址。
 */

function normalize(value: string) {
  return value.trim().replace(/\/+$/, "").toLowerCase();
}

function decode(value: string) {
  try {
    return decodeURI(value);
  } catch {
    return value;
  }
}

/** 标题是否只是地址的缩短形式（地址尾部就是标题，或标题就是域名）。 */
export function isShortenedLinkLabel(label: string, href: string) {
  const normalizedLabel = normalize(decode(label));
  const normalizedHref = normalize(decode(href));
  if (!normalizedLabel || !normalizedHref) return false;
  if (normalizedLabel === normalizedHref) return true;
  if (normalizedHref.endsWith(normalizedLabel)) return true;
  try {
    return normalizedLabel === normalize(new URL(href).host);
  } catch {
    return false;
  }
}

/**
 * 返回粘贴后应该出现在编辑器里的文本：完整地址，或 `[标题](地址)` 形式的 Markdown。
 * 返回空字符串表示该链接不应被转换（调用方回退为标题文本）。
 */
export function visibleLinkText(label: string, href: string) {
  const text = label.trim();
  if (!text) return href;
  return isShortenedLinkLabel(text, href) ? href : `[${text}](${href})`;
}