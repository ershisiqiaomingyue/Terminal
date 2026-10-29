import { marked } from "marked";

export function normalizeMarkdownStructure(source: string) {
  return source.replace(/\r\n?/g, "\n");
}

function openFence(source: string) {
  let active = "";
  for (const line of source.split("\n")) {
    const match = /^\s{0,3}(`{3,}|~{3,})/.exec(line);
    if (!match) continue;
    const marker = match[1];
    if (!active) active = marker;
    else if (marker[0] === active[0] && marker.length >= active.length) active = "";
  }
  return active;
}

function hasUnclosedStrong(source: string) {
  let strongMarkers = 0;
  let fence = "";
  let inlineTicks = 0;
  for (let index = 0; index < source.length;) {
    const lineStart = index === 0 || source[index - 1] === "\n";
    if (lineStart) {
      const match = /^\s{0,3}(`{3,}|~{3,})/.exec(source.slice(index));
      if (match) {
        const marker = match[1];
        if (!fence) fence = marker;
        else if (marker[0] === fence[0] && marker.length >= fence.length) fence = "";
      }
    }
    if (fence) {
      index += 1;
      continue;
    }
    if (source[index] === "`" && source[index - 1] !== "\\") {
      let run = 1;
      while (source[index + run] === "`") run += 1;
      inlineTicks = inlineTicks === run ? 0 : inlineTicks ? inlineTicks : run;
      index += run;
      continue;
    }
    if (!inlineTicks && source.startsWith("**", index) && source[index - 1] !== "\\") {
      strongMarkers += 1;
      index += 2;
      continue;
    }
    index += 1;
  }
  return strongMarkers % 2 === 1;
}

export function projectStreamingMarkdown(source: string) {
  const normalized = normalizeMarkdownStructure(source);
  const fence = openFence(normalized);
  if (fence) return `${normalized}${normalized.endsWith("\n") ? "" : "\n"}${fence}`;
  return hasUnclosedStrong(normalized) ? `${normalized}**` : normalized;
}

export function markdownToUnsafeHtml(source: string, options: { streaming?: boolean } = {}) {
  if (!source.trim()) return "";
  const projected = options.streaming ? projectStreamingMarkdown(source) : normalizeMarkdownStructure(source);
  return marked.parse(projected, {
    breaks: true,
    gfm: true,
  }) as string;
}
