import type { ChatSession, ExecutionTarget } from "./types.ts";

/**
 * 对话可以绑定的远程执行环境（SSH / WSL）。
 *
 * 绑定清单按优先级排列：第一项就是该对话的默认执行环境（无前缀工具使用它），
 * 其余主机则供带 `target` 参数的远程工具按需路由。
 * 绑定只写 `boundEnvironments`；`executionTarget` 仍保留「上次解析结果」的语义，
 * 由发送消息时盖章，避免两处状态互相覆盖。
 */
export const BOUND_ENVIRONMENTS_EVENT = "android-tool-bound-environments";

export type BindEnvironmentDetail = { sessionId: string; target: ExecutionTarget; bound: boolean };

/** 本机工作区属于项目本身，不是可绑定的远程环境。 */
export function bindableTarget(target: ExecutionTarget): boolean {
  return target.kind !== "local" && Boolean((target.id || "").trim());
}

export function environmentTargetKey(target: ExecutionTarget): string {
  return `${target.kind}:${(target.id || "").trim()}`;
}

/** 该对话已绑定的远程环境，按优先级排列。 */
export function sessionBoundEnvironments(
  session: Pick<ChatSession, "boundEnvironments"> | null | undefined,
): ExecutionTarget[] {
  const bound = session?.boundEnvironments;
  return Array.isArray(bound) ? bound.filter((target) => bindableTarget(target)) : [];
}

export function isSessionEnvironmentBound(
  session: Pick<ChatSession, "boundEnvironments"> | null | undefined,
  target: ExecutionTarget,
): boolean {
  const key = environmentTargetKey(target);
  return sessionBoundEnvironments(session).some((bound) => environmentTargetKey(bound) === key);
}

/** 增删绑定并保持顺序（新绑定追加在末尾，仍以首项为默认环境）。 */
export function updateSessionBoundEnvironments(
  sessions: ChatSession[],
  sessionId: string,
  target: ExecutionTarget,
  bound: boolean,
): ChatSession[] {
  if (!sessionId || !bindableTarget(target)) return sessions;
  const key = environmentTargetKey(target);
  let changed = false;
  const next = sessions.map((session) => {
    if (session.id !== sessionId) return session;
    const current = sessionBoundEnvironments(session);
    const exists = current.some((bound) => environmentTargetKey(bound) === key);
    if (bound === exists) return session;
    changed = true;
    const environments = bound
      ? [...current, target]
      : current.filter((bound) => environmentTargetKey(bound) !== key);
    return { ...session, boundEnvironments: environments };
  });
  return changed ? next : sessions;
}

export function dispatchBoundEnvironmentChange(detail: BindEnvironmentDetail) {
  window.dispatchEvent(new CustomEvent<BindEnvironmentDetail>(BOUND_ENVIRONMENTS_EVENT, { detail }));
}

/** 请求体里的执行目标形状（与 Rust 侧 `AgentExecutionTarget` 对应）。 */
export type ExecutionTargetPayload = {
  kind: string;
  connectionId: string | null;
  profileId: string | null;
  root: string | null;
};

function targetPayload(target: ExecutionTarget, root: string | null): ExecutionTargetPayload {
  return {
    kind: target.kind,
    connectionId: target.kind === "ssh" ? target.id ?? null : null,
    profileId: target.kind === "wsl" ? target.id ?? null : null,
    root,
  };
}

/**
 * 组装请求的 `executionTargets`：首项必须与该轮的 `executionTarget` 一致
 * （它就是默认执行环境），其余是该对话绑定的远程主机。
 *
 * 运行时据此为每台已连接的主机准备独立上下文，并给 `ssh__*` 工具注入可选
 * `target` 参数；绑定清单为空时返回空数组，旧行为完全不变。
 */
export function executionTargetsPayload(
  defaultTarget: ExecutionTarget,
  defaultRoot: string | null | undefined,
  bound: ExecutionTarget[],
): ExecutionTargetPayload[] {
  const payload = [targetPayload(defaultTarget, (defaultRoot || "").trim() || null)];
  const seen = new Set([environmentTargetKey(defaultTarget)]);
  for (const target of bound) {
    if (!bindableTarget(target)) continue;
    const key = environmentTargetKey(target);
    if (seen.has(key)) continue;
    seen.add(key);
    payload.push(targetPayload(target, (target.root || "").trim() || null));
  }
  return payload;
}
