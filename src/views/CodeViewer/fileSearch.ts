export const MAX_FILE_MATCHES = 1000;
export type FileSearchOptions = { caseSensitive?: boolean; wholeWord?: boolean; regex?: boolean };

export function findFileMatches(source: string, query: string, options: FileSearchOptions = {}) {
  const matches: { start: number; end: number }[] = [];
  const empty = { matches, truncated: false, error: "" };
  if (!query) return empty;
  let pattern: RegExp;
  try { pattern = new RegExp(options.regex ? query : query.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), options.caseSensitive ? "gu" : "giu"); }
  catch { return { ...empty, error: "正则表达式无效" }; }
  const wordCharacter = /[\p{L}\p{N}_]/u;
  for (const match of source.matchAll(pattern)) {
    if (!match[0].length) continue;
    if (options.wholeWord) {
      const before = Array.from(source.slice(Math.max(0, match.index - 2), match.index)).pop() || "";
      const after = Array.from(source.slice(match.index + match[0].length, match.index + match[0].length + 2))[0] || "";
      if (wordCharacter.test(before) || wordCharacter.test(after)) continue;
    }
    if (matches.length === MAX_FILE_MATCHES) return { matches, truncated: true, error: "" };
    matches.push({ start: match.index, end: match.index + match[0].length });
  }
  return { matches, truncated: false, error: "" };
}

export function highlightFileMatches(root: HTMLElement, query: string, options: FileSearchOptions = {}) {
  for (const mark of root.querySelectorAll("mark[data-file-match]")) mark.replaceWith(...mark.childNodes);
  root.normalize();
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const nodes: { node: Text; start: number; end: number }[] = [];
  let source = "";
  while (walker.nextNode()) {
    const node = walker.currentNode as Text;
    if (node.parentElement?.closest("[data-file-search-ignore]")) continue;
    nodes.push({ node, start: source.length, end: source.length + node.length });
    source += node.data;
  }
  const result = findFileMatches(source, query, options);
  let firstMatch = 0;
  // A match can cross syntax-highlight spans, so offsets are taken from the
  // combined visible text, then mapped back to each text node.
  for (const { node, start, end } of nodes) {
    while (firstMatch < result.matches.length && result.matches[firstMatch].end <= start) firstMatch += 1;
    const fragment = document.createDocumentFragment();
    let cursor = 0;
    for (let index = firstMatch; index < result.matches.length && result.matches[index].start < end; index += 1) {
      const match = result.matches[index];
      const from = Math.max(start, match.start) - start;
      const to = Math.min(end, match.end) - start;
      fragment.append(node.data.slice(cursor, from));
      const mark = document.createElement("mark");
      mark.dataset.fileMatch = String(index);
      mark.textContent = node.data.slice(from, to);
      fragment.append(mark);
      cursor = to;
    }
    if (!cursor) continue;
    fragment.append(node.data.slice(cursor));
    node.replaceWith(fragment);
  }
  return result;
}
