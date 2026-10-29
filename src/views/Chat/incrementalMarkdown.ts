import { marked } from "marked";

// Adapted to marked from DeepSeek Harness's block-freezing strategy.
// Source: ui-primitives/src/markdown/incremental.ts (639ed01), MIT.
export type MarkdownPart = { offset: number; source: string; streaming: boolean };

export class IncrementalMarkdown {
  private previous = "";
  private offset = 0;
  private frozen: MarkdownPart[] = [];
  private parts: MarkdownPart[] = [];

  update(source: string): MarkdownPart[] {
    if (source === this.previous) return this.parts;
    if (!source.startsWith(this.previous)) {
      this.offset = 0;
      this.frozen = [];
    }
    this.previous = source;
    const tail = source.slice(this.offset);
    const tokens = marked.lexer(tail, { gfm: true });
    // Definitions resolve document-wide. Keep them in a full parse, including
    // any references that precede a definition arriving in a later chunk.
    if (Object.keys(tokens.links).length || /^ {0,3}\[[^\]\n]+\]:/m.test(source)) {
      this.offset = 0;
      this.frozen = [];
      this.parts = [{ offset: 0, source, streaming: true }];
      return this.parts;
    }
    const blocks = tokens.filter((token) => token.type !== "space");
    const freezeCount = Math.max(0, blocks.length - 2);
    let remaining = freezeCount;
    let consumed = 0;
    for (const token of tokens) {
      if (remaining === 0) break;
      if (token.type !== "space") {
        this.frozen.push({ offset: this.offset + consumed, source: token.raw, streaming: false });
        remaining -= 1;
      }
      consumed += token.raw.length;
    }
    this.offset += consumed;
    this.parts = [...this.frozen, { offset: this.offset, source: source.slice(this.offset), streaming: true }];
    return this.parts;
  }
}
