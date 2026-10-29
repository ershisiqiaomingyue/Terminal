/**
 * 修复审批卡片（设计文档 9.4 第 3 步 / T4-7）。
 *
 * 职责只有一个：把 `repairSession` 的状态画成一张"看得懂再决定"的卡，
 * 并把用户动作原样上报给父级。它**不执行命令**、不碰 PTY、不发请求——
 * 所有副作用都在 `index.tsx` 里，这样卡片本身可以纯粹按状态渲染。
 *
 * 三道防线在 UI 上的呈现：
 * 1. 风险等级用本地重算的结果展示（模型自报的等级不采信）；
 * 2. `destructive` 显示"手打命令词"输入框，没打对按钮就是灰的（9.4 安全约束）；
 * 3. 中断按钮任何进行中的阶段都在，用户随时能停。
 */
import React from "react";
import { AlertTriangle, CheckCircle2, Loader2, ShieldAlert, Wrench, X } from "lucide-react";
import { Button } from "@/components/ui";
import {
  MAX_REPAIR_ROUNDS,
  repairConfirmToken,
  repairExecutable,
  repairNeedsTyped,
  type RepairState,
} from "./repairSession";

export interface RepairCardProps {
  state: RepairState;
  /** 触发修复的那条失败命令，用于标题。 */
  blockCommand?: string;
  /** 父级正在跑异步任务（生成中 / 执行中），用于禁用按钮。 */
  busy?: boolean;
  onTyped: (value: string) => void;
  onApprove: () => void;
  onReject: () => void;
  onAbort: () => void;
  onClose: () => void;
}

const RISK_LABEL = {
  "read-only": "只读",
  mutating: "会改状态",
  destructive: "破坏性",
} as const;

export const RepairCard: React.FC<RepairCardProps> = ({
  state,
  blockCommand,
  busy,
  onTyped,
  onApprove,
  onReject,
  onAbort,
  onClose,
}) => {
  const pending = state.pending;
  const round = state.rounds.length + (state.phase === "awaiting" ? 1 : 0);
  const inFlight = state.phase === "diagnosing" || state.phase === "running";
  const executable = repairExecutable(state);
  const needsTyped = repairNeedsTyped(state);
  const token = repairConfirmToken(state);

  return (
    <div className="repair-card" role="region" aria-label="自动修复">
      <div className="repair-card-head">
        <div className="repair-card-title">
          <Wrench size={14} />
          <span>尝试修复</span>
          {blockCommand ? <code className="repair-card-failed" title={blockCommand}>{blockCommand}</code> : null}
        </div>
        <div className="repair-card-head-actions">
          <span className="repair-card-round">
            第 {Math.min(Math.max(round, 1), MAX_REPAIR_ROUNDS)}/{MAX_REPAIR_ROUNDS} 轮
          </span>
          <Button
            variant="bare"
            size="xs"
            icon={<X size={12} />}
            aria-label="关闭修复卡片"
            title="关闭（不会中断已在运行的命令）"
            onClick={onClose}
          />
        </div>
      </div>

      <div className="repair-card-body">
        {state.phase === "diagnosing" && (
          <div className="repair-card-status">
            <Loader2 size={14} className="repair-card-spin" />
            <span>正在按诊断生成修复命令…</span>
          </div>
        )}

        {state.phase === "awaiting" && pending && (
          <>
            <div className="repair-card-cmd">
              <code>{pending.command}</code>
              <span className="repair-card-risk" data-risk={pending.risk}>
                {pending.risk === "destructive" ? <ShieldAlert size={12} /> : <AlertTriangle size={12} />}
                {RISK_LABEL[pending.risk]}
              </span>
            </div>
            {pending.explanation ? <p className="repair-card-note">{pending.explanation}</p> : null}

            {needsTyped && (
              <div className="repair-card-typed">
                <label htmlFor="repair-typed">
                  破坏性操作：请手打 <code>{token}</code> 确认（不会自动执行）
                </label>
                <input
                  id="repair-typed"
                  className="repair-card-typed-input"
                  value={state.typed}
                  placeholder={token}
                  autoComplete="off"
                  spellCheck={false}
                  onChange={(event) => onTyped(event.target.value)}
                />
              </div>
            )}

            <div className="repair-card-actions">
              <Button
                variant="secondary"
                size="sm"
                disabled={busy || !executable}
                title={executable ? "把命令写入终端并执行" : `先手打 ${token} 确认`}
                onClick={onApprove}
              >
                批准执行
              </Button>
              <Button variant="bare" size="sm" onClick={onReject} disabled={busy}>
                放弃这条
              </Button>
            </div>
          </>
        )}

        {state.phase === "running" && (
          <div className="repair-card-status">
            <Loader2 size={14} className="repair-card-spin" />
            <span>已写入终端，等待这条命令的结果…</span>
          </div>
        )}

        {state.phase === "done" && (
          <div className="repair-card-status" data-ok="true">
            <CheckCircle2 size={14} />
            <span>{state.message ?? "修复成功。"}</span>
          </div>
        )}

        {(state.phase === "idle" || state.phase === "exhausted" || state.phase === "aborted") && state.message ? (
          <div className="repair-card-status">{state.message}</div>
        ) : null}

        {state.rounds.length > 0 && (
          <ol className="repair-card-history">
            {state.rounds.map((item) => (
              <li key={`${item.round}-${item.command}`}>
                <code>{item.command}</code>
                <em data-outcome={item.outcome ?? "pending"}>
                  {item.outcome === "success" ? "成功" : item.outcome === "failed" ? "失败" : "执行中"}
                </em>
              </li>
            ))}
          </ol>
        )}
      </div>

      {inFlight && (
        <div className="repair-card-foot">
          <Button variant="bare" size="xs" onClick={onAbort}>
            中断修复
          </Button>
        </div>
      )}
    </div>
  );
};

export default RepairCard;
