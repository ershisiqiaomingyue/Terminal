import { marked, type Token, type TokensList } from "marked";
import { normalizeMarkdownStructure, projectStreamingMarkdown } from "./markdownProjection.ts";

export type LongMarkdownBlock = { kind: "paragraph"; chunks: string[] } | { kind: "html"; html: string };
export type LongMarkdownRequest = { source: string; streaming: boolean };
export type LongMarkdownResult = LongMarkdownRequest & { blocks: LongMarkdownBlock[] };

/** Keep an already displayed streaming preview until the final parse arrives. */
export function canPreviewLongMarkdown(result: LongMarkdownResult, request: LongMarkdownRequest): boolean {
  return request.source.startsWith(result.source)
    && (result.streaming === request.streaming || (result.streaming && !request.streaming));
}

const CHUNK_SIZE = 4096;
const OPTIONS = { gfm: true, breaks: true };

function inlineChunks(tokens: Token[]) {
  const chunks: string[] = [];
  let chunk = "";
  const append = (html: string) => {
    if (chunk && chunk.length + html.length > CHUNK_SIZE) { chunks.push(chunk); chunk = ""; }
    chunk += html;
    if (chunk.length >= CHUNK_SIZE) { chunks.push(chunk); chunk = ""; }
  };
  for (const token of tokens) {
    if ((token.type === "strong" || token.type === "em" || token.type === "del") && token.tokens && token.raw.length > CHUNK_SIZE) {
      const tag = token.type === "em" ? "em" : token.type;
      for (const html of inlineChunks(token.tokens)) append(`<${tag}>${html}</${tag}>`);
    } else if (token.type === "text" && !token.tokens && token.text.length > CHUNK_SIZE) {
      for (let start = 0; start < token.text.length;) {
        let end = Math.min(start + CHUNK_SIZE, token.text.length);
        if (end < token.text.length) {
          // Prefer whitespace so paths and entities aren't split across spans.
          const space = token.text.lastIndexOf(" ", end);
          if (space > start) end = space + 1;
          else {
            const entity = token.text.lastIndexOf("&", end - 1);
            if (entity > start && /^&(?:#\w*|\w*)$/.test(token.text.slice(entity, end))) end = entity;
            if (/[\uD800-\uDBFF]/.test(token.text[end - 1])) end -= 1;
          }
        }
        const text = token.text.slice(start, end);
        append(marked.Parser.parseInline([{ ...token, raw: text, text }], OPTIONS));
        start = end;
      }
    } else append(marked.Parser.parseInline([token], OPTIONS));
  }
  if (chunk) chunks.push(chunk);
  return chunks;
}

function hasInlineHtml(tokens: Token[]): boolean {
  return tokens.some((token) => token.type === "html" || ("tokens" in token && Boolean(token.tokens && hasInlineHtml(token.tokens))));
}

export function projectLongMarkdown({ source, streaming }: LongMarkdownRequest): LongMarkdownResult {
  const projected = streaming ? projectStreamingMarkdown(source) : normalizeMarkdownStructure(source);
  const tokens = marked.lexer(projected, OPTIONS);
  const blocks: LongMarkdownBlock[] = [];
  for (const token of tokens) {
    if (token.type === "space" || token.type === "def") continue;
    // Raw HTML can span inline tokens. Sanitize that block together instead
    // of changing its nesting by placing React spans inside it.
    if (token.type === "paragraph" && token.tokens && !hasInlineHtml(token.tokens)) {
      blocks.push({ kind: "paragraph", chunks: inlineChunks(token.tokens) });
    } else {
      const blockTokens = Object.assign([token], { links: tokens.links }) as TokensList;
      blocks.push({ kind: "html", html: marked.parser(blockTokens, OPTIONS) });
    }
  }
  return { source, streaming, blocks };
}
