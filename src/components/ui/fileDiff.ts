export type FileDiffRow = { kind: "added" | "removed" | "context" | "meta"; text: string; oldLine?: number; newLine?: number };
export const MAX_DIFF_ROWS = 4000;

// Consume unified hunks only: file headers must never count as added/deleted code.
export function parseFileDiff(diff: string) {
  const rows: FileDiffRow[] = [];
  let oldLine = 0, newLine = 0, oldRemaining = 0, newRemaining = 0;
  let added = 0, removed = 0, truncated = false;
  for (const line of diff.split(/\r?\n/)) {
    const hunk = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/.exec(line);
    let row: FileDiffRow;
    if (hunk) {
      oldLine = Number(hunk[1]); newLine = Number(hunk[3]);
      oldRemaining = Number(hunk[2] ?? 1); newRemaining = Number(hunk[4] ?? 1);
      row = { kind: "meta", text: line };
    } else if (line.startsWith("+") && newRemaining > 0) {
      added += 1; newRemaining -= 1;
      row = { kind: "added", text: line.slice(1), newLine: newLine++ };
    } else if (line.startsWith("-") && oldRemaining > 0) {
      removed += 1; oldRemaining -= 1;
      row = { kind: "removed", text: line.slice(1), oldLine: oldLine++ };
    } else if (line.startsWith(" ") && oldRemaining > 0 && newRemaining > 0) {
      oldRemaining -= 1; newRemaining -= 1;
      row = { kind: "context", text: line.slice(1), oldLine: oldLine++, newLine: newLine++ };
    } else {
      if (!line) continue;
      if (!line.startsWith("\\")) oldRemaining = newRemaining = 0;
      row = { kind: "meta", text: line };
    }
    if (rows.length < MAX_DIFF_ROWS) rows.push(row); else truncated = true;
  }
  return { rows, added, removed, truncated, partial: truncated || /^(?:…|\.\.\.).*(?:截断|truncated)/im.test(diff) };
}
