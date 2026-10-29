export interface TerminalTextBuffer {
  chunks: string[];
  start: number;
  length: number;
}

export function getTerminalTextBuffer(
  buffers: Record<string, TerminalTextBuffer>,
  id: string,
): TerminalTextBuffer {
  return buffers[id] ||= { chunks: [], start: 0, length: 0 };
}

/** Output can arrive before open_terminal_session returns its id. Keep this
 * short handshake cache bounded even when other windows are producing output. */
export function bufferEarlyTerminalOutput(buffers: Record<string, TerminalTextBuffer>, id: string, data: string) {
  if (!buffers[id]) {
    const ids = Object.keys(buffers);
    if (ids.length >= 8) delete buffers[ids[0]];
  }
  appendTerminalText(getTerminalTextBuffer(buffers, id), data, 128_000);
}

function compactTerminalTextBuffer(buffer: TerminalTextBuffer) {
  if (buffer.start === 0 || buffer.start < 64 && buffer.start * 2 < buffer.chunks.length) return;
  buffer.chunks = buffer.chunks.slice(buffer.start);
  buffer.start = 0;
}

export function clearTerminalTextBuffer(buffer: TerminalTextBuffer | undefined) {
  if (!buffer) return;
  buffer.chunks = [];
  buffer.start = 0;
  buffer.length = 0;
}

export function appendTerminalText(buffer: TerminalTextBuffer, text: string, maxLength = Number.POSITIVE_INFINITY) {
  if (!text) return;
  buffer.chunks.push(text);
  buffer.length += text.length;

  while (buffer.length > maxLength && buffer.start < buffer.chunks.length) {
    const overflow = buffer.length - maxLength;
    const first = buffer.chunks[buffer.start];
    if (first.length <= overflow) {
      buffer.length -= first.length;
      buffer.start += 1;
    } else {
      buffer.chunks[buffer.start] = first.slice(overflow);
      buffer.length -= overflow;
    }
  }
  compactTerminalTextBuffer(buffer);
}

export function readTerminalText(buffer: TerminalTextBuffer | undefined) {
  if (!buffer?.length) return "";
  return buffer.chunks.slice(buffer.start).join("");
}

export function takeTerminalText(buffer: TerminalTextBuffer | undefined, maxLength = Number.POSITIVE_INFINITY) {
  if (!buffer?.length || maxLength <= 0) return "";
  const chunks: string[] = [];
  let remaining = maxLength;

  while (buffer.start < buffer.chunks.length && remaining > 0) {
    const chunk = buffer.chunks[buffer.start];
    if (chunk.length <= remaining) {
      chunks.push(chunk);
      remaining -= chunk.length;
      buffer.length -= chunk.length;
      buffer.start += 1;
      continue;
    }
    chunks.push(chunk.slice(0, remaining));
    buffer.chunks[buffer.start] = chunk.slice(remaining);
    buffer.length -= remaining;
    remaining = 0;
  }

  compactTerminalTextBuffer(buffer);
  return chunks.join("");
}

export function isImmediateTerminalInput(input: string) {
  return /[\r\n\u0003\u001b]/.test(input);
}
