export type AospConnectionPhase = "idle" | "checking" | "connected" | "failed" | "paused";
export type AospConnectionSnapshot = { key: string; phase: AospConnectionPhase; error: string };

export function aospGlobalSshState(id: string, selectedId: string | undefined, status: string | undefined, error = ""): AospConnectionSnapshot {
  const key = `ssh:${id}`;
  if (!id || id !== selectedId) return { key, phase: "idle", error: "" };
  const phase = status === "connected" ? "connected" : status === "connecting" ? "checking"
    : status === "failed" ? "failed" : status === "disconnected" ? "paused" : "idle";
  return { key, phase, error };
}
type Target = { kind: "ssh" | "wsl"; id: string };
type Dependencies = { check: (target: Target) => Promise<void>; paused: (target: Target) => boolean; errorText: (error: unknown) => string };

/** Discard late checks after target switches or an explicit disconnect. */
export class AospConnectionController {
  private state: AospConnectionSnapshot = { key: "", phase: "idle", error: "" };
  private generation = 0;
  private pending: { key: string; promise: Promise<boolean> } | undefined;
  private listeners = new Set<() => void>();
  private dependencies: Dependencies;
  constructor(dependencies: Dependencies) { this.dependencies = dependencies; }
  getSnapshot = () => this.state;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private publish(state: AospConnectionSnapshot) { this.state = state; this.listeners.forEach((listener) => listener()); }
  invalidate() { this.generation++; this.pending = undefined; }
  pause(target: Target) { this.invalidate(); this.publish({ key: `${target.kind}:${target.id}`, phase: "paused", error: "" }); }
  check(target: Target, refresh = false): Promise<boolean> {
    const key = `${target.kind}:${target.id}`;
    if (!target.id) { this.invalidate(); this.publish({ key, phase: "idle", error: "" }); return Promise.resolve(false); }
    if (this.dependencies.paused(target)) { this.pause(target); return Promise.resolve(false); }
    if (this.pending?.key === key) return this.pending.promise;
    if (!refresh && this.state.key === key && this.state.phase === "connected") return Promise.resolve(true);
    const generation = ++this.generation;
    this.publish({ key, phase: "checking", error: "" });
    const promise = this.dependencies.check(target).then(() => {
      if (generation !== this.generation || this.dependencies.paused(target)) return false;
      this.publish({ key, phase: "connected", error: "" }); return true;
    }, (error) => {
      if (generation === this.generation && !this.dependencies.paused(target)) this.publish({ key, phase: "failed", error: this.dependencies.errorText(error) });
      return false;
    }).finally(() => { if (this.pending?.promise === promise) this.pending = undefined; });
    this.pending = { key, promise };
    return promise;
  }
}
