export interface TerminalChunk { sequence: number; data: string }
export interface TerminalOutputEvent extends TerminalChunk { sessionId: string }

/** Merge replay and live output in sequence order during window handoff. */
export class TerminalHandoff {
  sequences: Record<string, number>;
  private pending = new Map<string, TerminalChunk[]>();
  private pendingCharacters = new Map<string, number>();
  constructor(sequences: Record<string, number> = {}) {
    this.sequences = { ...sequences };
    for (const id of Object.keys(sequences)) this.pending.set(id, []);
  }
  accept(event: TerminalOutputEvent, deliver: (data: string) => void) {
    const pending = this.pending.get(event.sessionId);
    if (pending) {
      pending.push(event);
      let characters = (this.pendingCharacters.get(event.sessionId) || 0) + event.data.length;
      // Match the native replay budget while IPC recovery is pending.
      while ((characters > 512 * 1024 || pending.length > 1024) && pending.length > 1) characters -= pending.shift()!.data.length;
      this.pendingCharacters.set(event.sessionId, characters);
      return;
    }
    if (event.sequence <= (this.sequences[event.sessionId] || 0)) return;
    this.sequences[event.sessionId] = event.sequence;
    deliver(event.data);
  }
  recover(id: string, replay: TerminalChunk[], deliver: (data: string) => void) {
    const chunks = [...replay, ...(this.pending.get(id) || [])].sort((a, b) => a.sequence - b.sequence);
    this.pending.delete(id);
    this.pendingCharacters.delete(id);
    for (const chunk of chunks) this.accept({ ...chunk, sessionId: id }, deliver);
  }

  retainSessions(ids: Iterable<string>) {
    const keep = new Set(ids);
    for (const id of Object.keys(this.sequences)) if (!keep.has(id)) delete this.sequences[id];
    for (const id of this.pending.keys()) if (!keep.has(id)) {
      this.pending.delete(id);
      this.pendingCharacters.delete(id);
    }
  }
}
