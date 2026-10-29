/** Follow the current line while active; settled previews start at the beginning. */
export function reasoningPreviewText(content: string, active = false, fallback = "思考中…") {
  const source = active ? content.trimEnd() : content.trimStart();
  const line = (active
    ? source.slice(source.lastIndexOf("\n") + 1)
    : source.slice(0, source.indexOf("\n") < 0 ? source.length : source.indexOf("\n"))).trim();
  const text = line.replace(/^\s{0,3}(?:#{1,6}\s+|>\s*|[-*+]\s+|\d+[.)]\s+)/, "");
  // Bounded preview work also keeps long reasoning from becoming a giant DOM text node.
  const characters = Array.from(text.slice(0, 402));
  const head = characters.slice(0, 200).join("");
  return head ? `${head}${characters.length > 200 ? "…" : ""}` : fallback;
}
