/** Replaying a terminal transcript must not answer old device-status queries. */
export function sanitizeTerminalReplay(text: string) {
  return text.replace(/\x1b\[(?:[56]n|[>0]?c)/g, "");
}

export function isTerminalQueryResponse(text: string) {
  return /^(?:\x1b\[(?:\d+;\d+R|[?>]?[\d;]*c|[03]n))+$/.test(text);
}
