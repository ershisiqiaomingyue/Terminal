const LOCAL_PATH_PATTERN = /(?:[A-Za-z]:[\\/]|\\\\)[^\s<>"'`，。；：！？]+[，。；：！？]*|\/(?:[\w.-]+\/)+[\w.-]+\/*/g;
const TRAILING_PATH_PUNCTUATION = /[),.;!?，。；：！？]+$/u;

/** Keep directory separators inside the path and prose punctuation outside. */
export function localPathMatches(text: string) {
  return Array.from(text.matchAll(LOCAL_PATH_PATTERN)).flatMap((match) => {
    const raw = match[0];
    const trailing = raw.match(TRAILING_PATH_PUNCTUATION)?.[0] || "";
    const path = trailing ? raw.slice(0, -trailing.length) : raw;
    // Chinese prose may directly precede a path without a separating space.
    if (/[A-Za-z0-9_./:\\-]/.test(text[match.index - 1] || "")) return [];
    if (path.startsWith("/") && !path.startsWith("//")) {
      // Slash-separated numbers are lists/dates/fractions, not file references.
      // Also avoid decorating the suffix of URLs and relative paths.
      if (/^\/[\d./-]+$/.test(path)) return [];
    }
    return [{
      index: match.index,
      end: match.index + raw.length,
      path,
      trailing,
    }];
  });
}
