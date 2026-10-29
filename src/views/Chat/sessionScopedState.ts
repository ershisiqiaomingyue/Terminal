import type { ChatSession, ChatSource, ExecutionTarget } from "./types";
import { environmentTargetKey, sessionBoundEnvironments } from "./conversationEnvironments.ts";

/** A project owns its working directory; a connected global SSH is only an
 * available environment, not permission to move that project to another host. */
export function conversationExecutionTarget(
  workspacePath: string,
  session: Pick<ChatSession, "workspacePath" | "executionTarget" | "boundEnvironments"> | null,
  globalTarget: ExecutionTarget,
): ExecutionTarget {
  // 显式绑定的远程环境优先：绑定是用户对本对话的明确意图，
  // 即使工作区是盘符/UNC 路径也不再退回本机。
  const bound = sessionBoundEnvironments(session)[0];
  if (bound) return { ...bound };
  const root = (session?.workspacePath || workspacePath).trim();
  if (!root) return globalTarget;
  // Also repairs older sessions which saved a Windows project together with
  // the global SSH target. Never send a drive/UNC path to a remote tool.
  if (/^(?:[a-z]:[\\/]|\\\\|\/\/)/i.test(root)) {
    return { kind: "local", label: "本机工作区", root };
  }
  const saved = session?.executionTarget;
  return saved ? { ...saved, root } : { ...globalTarget, root };
}

export function conversationEnvironmentError(
  target: ExecutionTarget,
  globalTarget: ExecutionTarget,
  globalError: string,
  boundTargets: ExecutionTarget[] = [],
): string {
  // Native SSH preparation asks through the conversation's existing question
  // card. Let the request reach it, preserving the session's exact target.
  if (target.kind === "local" || target.kind === "ssh") return "";
  // 显式绑定过的环境由后端按绑定信息自己准备，不再要求菜单栏先选中。
  const key = environmentTargetKey(target);
  if (boundTargets.some((bound) => environmentTargetKey(bound) === key)) return "";
  if (target.kind !== globalTarget.kind || target.id !== globalTarget.id) {
    return "请在菜单栏连接此项目的 WSL 环境后重试。";
  }
  return globalError;
}

export function updateSessionSources(sessions: ChatSession[], sessionId: string, sources: ChatSource[]) {
  return sessions.map((session) => session.id === sessionId && session.sources !== sources
    ? { ...session, sources }
    : session);
}

export function updateSessionTarget(sessions: ChatSession[], sessionId: string, target: ExecutionTarget) {
  return sessions.map((session) => {
    if (session.id !== sessionId) return session;
    const current = session.executionTarget;
    if (current?.kind === target.kind
      && current.id === target.id
      && current.label === target.label
      && current.root === target.root) return session;
    return { ...session, executionTarget: target };
  });
}

export function updateSessionModelSelection(
  sessions: ChatSession[],
  sessionId: string,
  selection: { providerId: string; modelId: string },
) {
  return sessions.map((session) => {
    if (session.id !== sessionId) return session;
    if (session.modelSelection?.providerId === selection.providerId
      && session.modelSelection?.modelId === selection.modelId) return session;
    return { ...session, modelSelection: selection };
  });
}
