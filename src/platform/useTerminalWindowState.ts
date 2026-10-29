import { useRef } from "react";
import { readTerminalText, type TerminalTextBuffer } from "./terminalBuffer";
import { TerminalHandoff, type TerminalOutputEvent } from "./terminalHandoff";
import { readToolWindowState, useToolWindowState } from "./toolWindowState";
import { invoke } from "./runtime";
import { sanitizeTerminalReplay } from "./terminalReplay";

export interface TerminalWindowSnapshot<T> {
  tabs: T[];
  activeId: string;
  buffers: Record<string, string>;
  sequences: Record<string, number>;
}

export function useTerminalWindowState<T extends { id: string; sessionId?: string }>(key: string, tabs: T[], activeId: string, buffers: Record<string, TerminalTextBuffer>, opening: boolean | (() => boolean) = false, sessionIds?: () => string[]) {
  const initial = useRef(readToolWindowState<TerminalWindowSnapshot<T>>(key));
  const handoff = useRef<TerminalHandoff>();
  if (!handoff.current) handoff.current = new TerminalHandoff(initial.current?.sequences);
  const deliverRef = useRef<(event: TerminalOutputEvent) => void>(() => undefined);
  const scope = useRef({ opening, sessionIds, tabs });
  scope.current = { opening, sessionIds, tabs };
  useToolWindowState(key, () => {
    if (typeof opening === "function" ? opening() : opening) throw new Error("终端正在连接，请连接完成后再拖出。");
    return {
      tabs, activeId, sequences: Object.fromEntries(tabs.filter((tab) => tab.sessionId).map((tab) => [tab.sessionId!, handoff.current!.sequences[tab.sessionId!] || 0])),
      buffers: Object.fromEntries(tabs.map((tab) => [tab.id, readTerminalText(buffers[tab.id])])),
    };
  });
  return {
    accept(event: TerminalOutputEvent, deliver: (event: TerminalOutputEvent) => void) {
      const current = scope.current;
      const connecting = typeof current.opening === "function" ? current.opening() : current.opening;
      const ids = current.sessionIds?.() || current.tabs.flatMap(tab => tab.sessionId ? [tab.sessionId] : []);
      if (!connecting) {
        handoff.current!.retainSessions(ids);
        if (!ids.includes(event.sessionId)) return;
      }
      deliverRef.current = deliver;
      handoff.current!.accept(event, (data) => deliver({ ...event, data }));
    },
    retainSessions(ids: Iterable<string>) { handoff.current!.retainSessions(ids); },
    /** 各会话已投递到的 journal 序列号（页面切换缓存要用它补投离开期间的输出）。 */
    sequences() { return { ...handoff.current!.sequences }; },
    async recover(deliver: (event: TerminalOutputEvent) => void) {
      deliverRef.current = deliver;
      await Promise.all(Object.entries(initial.current?.sequences || {}).map(async ([sessionId, afterSequence]) => {
        let replay: TerminalOutputEvent[] = [];
        // 会话在后端已经不在了（应用重启过、上游关掉了）：整批补投不能因此失败，
        // 否则一个死会话就会让**所有**页签的离开期间输出都投不出来。跳过它，
        // 由调用方按页签维度决定是重连还是提示。
        try { replay = await invoke<TerminalOutputEvent[]>("read_terminal_output", { sessionId, afterSequence }); }
        catch { return; }
        handoff.current!.recover(sessionId, replay, (data) => deliverRef.current({ sessionId, data: sanitizeTerminalReplay(data), sequence: 0 }));
      }));
    },
  };
}
