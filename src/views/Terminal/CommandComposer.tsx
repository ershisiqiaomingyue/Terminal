/**
 * 输入侧 AI 面板（开发方案 9.3 / T4-1）。
 *
 * 与 `BlockAiPanel` 的分工：
 * - 那个是**输出侧**（贴在某条已执行命令下面，解释/诊断）；
 * - 这个是**输入侧**（自然语言 → 命令草稿），所以是独立输入区，不劫持 PS1。
 *
 * 三条安全口径都在这里落地：
 * 1. 草稿可编辑，但**编辑后风险重算**（见 `composerState.ts`）；
 * 2. `mutating` 要点击确认，`destructive` 要手打命令首词（10.1 第二层）；
 * 3. 面板只把命令文本交给父级，**执行与否由父级写 PTY**，本组件不碰 PTY。
 */
import React, { useCallback, useEffect, useMemo, useReducer, useRef } from "react";
import { AlertCircle, CornerDownLeft, ShieldAlert, Sparkles, Wand2, X } from "lucide-react";
import { Button, Spinner } from "rain-ui";
import { terminalAiDeps } from "@/platform/terminal/ai/terminalAiDeps.ts";
import { confirmationToken, draftAsMultiline } from "@/platform/terminal/ai/commandComposer.ts";
import type { ComposerTarget } from "@/platform/terminal/ai/commandComposer.ts";
import { draftCommands, extractNaturalLanguage } from "@/platform/terminal/ai/commandComposerService.ts";
import type { TerminalModelSelection } from "@/platform/terminal/ai/terminalAiService.ts";
import {
  composerHint,
  composerReducer,
  executeGate,
  initialComposerState,
  needsAttention,
  remainingCount,
} from "./composerState";

export interface CommandComposerProps {
  /** 当前页签的目标类型（决定提示词里的执行环境）。 */
  target: ComposerTarget;
  /** 当前工作目录（未知时省略）。 */
  cwd?: string;
  /** 最近的命令历史（最新在前），用于提示词与「全部插入」。 */
  history?: string[];
  /** 直通模式提示；非空表示面板必须禁用（9.3.4）。 */
  passthroughHint?: string | null;
  /**
   * 从底部指令栏带过来的自然语言：打开即预填并**立刻发起一次生成**。
   *
   * 带初值只在挂载时取一次（`useReducer` 的惰性初始化），后续不受控，
   * 否则用户手动改内容后会被父级的旧值覆盖。
   */
  initialNl?: string;
  selection?: TerminalModelSelection;
  /** 执行：父级把命令写进 PTY 并回车。 */
  onExecute: (command: string) => void;
  /** 只插入不执行（Esc/Tab 的 Tab 分支）。 */
  onInsertOnly: (command: string) => void;
  /** 多命令「全部插入」（9.3.3）。 */
  onInsertAll: (multiline: string) => void;
  onClose: () => void;
}

const RISK_LABEL = {
  "read-only": "只读",
  mutating: "会改状态",
  destructive: "破坏性",
} as const;

export const CommandComposer: React.FC<CommandComposerProps> = ({
  target,
  cwd,
  history,
  passthroughHint,
  initialNl,
  selection,
  onExecute,
  onInsertOnly,
  onInsertAll,
  onClose,
}) => {
  const prefilled = (initialNl ?? "").trim();
  const [state, dispatch] = useReducer(composerReducer, prefilled, (value) =>
    value ? { ...initialComposerState(), nl: value } : initialComposerState(),
  );
  const abortRef = useRef<AbortController | null>(null);
  const runIdRef = useRef(0);
  const mountedRef = useRef(true);
  const nlRef = useRef<HTMLInputElement | null>(null);
  const draftRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      abortRef.current?.abort();
    };
  }, []);

  // 打开即把焦点放到自然语言行，避免用户先点一下才生效。
  useEffect(() => {
    nlRef.current?.focus();
  }, []);

  useEffect(() => {
    if (state.phase === "ready") draftRef.current?.focus();
  }, [state.phase]);

  const blocked = Boolean(passthroughHint);

  const run = useCallback(async () => {
    if (blocked) return;
    // 允许用户带 `#` 前缀（沿用 9.3.1 的书写习惯），也允许直接写。
    const nl = extractNaturalLanguage(state.nl) ?? state.nl.trim();
    if (!nl) {
      dispatch({ type: "failure", message: "先在输入框里写一句需求，例如「看一下电池温度」" });
      return;
    }

    abortRef.current?.abort();
    const controller = new AbortController();
    abortRef.current = controller;
    const runId = ++runIdRef.current;
    dispatch({ type: "start" });

    const deps = terminalAiDeps();

    const outcome = await draftCommands(
      { nl, target, cwd, history },
      deps,
      { selection, signal: controller.signal },
    );
    if (runIdRef.current !== runId || !mountedRef.current) return;

    if (outcome.ok) {
      dispatch({
        type: "success",
        draft: outcome.draft,
        meta: { modelId: outcome.modelId, durationMs: outcome.durationMs, requestBytes: outcome.requestBytes },
      });
    } else {
      dispatch({ type: "failure", message: outcome.message });
    }
  }, [blocked, cwd, history, selection, state.nl, target]);

  // 从底部指令栏带自然语言进来时，挂载后直接跑一次，省掉"再点一次生成"。
  // 只在挂载时触发：`run` 的依赖会随用户编辑变化，放进依赖数组会重复请求。
  const autoRanRef = useRef(false);
  useEffect(() => {
    if (autoRanRef.current) return;
    autoRanRef.current = true;
    if (!prefilled || blocked) return;
    void run();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const gate = executeGate(state);

  const execute = () => {
    if (!gate.ok) return;
    onExecute(state.editable.trim());
    if (remainingCount(state) === 0) {
      // 没有下一条了，面板使命结束，把焦点还给终端。
      onClose();
      return;
    }
    // 有剩余命令：推进到下一条，确认态在 reducer 里被重置（9.3.3 逐条确认）。
    dispatch({ type: "advance" });
  };

  const insertOnly = () => {
    const command = state.editable.trim();
    if (!command) return;
    onInsertOnly(command);
    onClose();
  };

  const insertAll = () => {
    const multiline = draftAsMultiline(
      {
        commands: state.commands,
        explanation: state.explanation,
        risk: state.risk,
        needsRoot: state.needsRoot,
        riskEscalated: state.riskEscalated,
      },
      "\n",
    );
    onInsertAll(multiline);
    onClose();
  };

  const expectedToken = useMemo(
    () => (state.risk === "destructive" ? confirmationToken(state.editable) : ""),
    [state.editable, state.risk],
  );

  const busy = state.phase === "drafting";

  return (
    <div
      className="command-composer"
      data-phase={state.phase}
      data-risk={state.phase === "ready" ? state.risk : undefined}
      role="region"
      aria-label="自然语言输入"
    >
      <header className="command-composer-head">
        <div className="command-composer-title">
          <Wand2 size={14} />
          <span>用一句话描述要做什么</span>
        </div>
        <Button variant="bare" icon={<X size={14} />} aria-label="关闭" title="关闭（Esc）" onClick={onClose} />
      </header>

      {blocked && (
        <div className="command-composer-blocked" role="status">
          <AlertCircle size={14} />
          <span>{passthroughHint}</span>
        </div>
      )}

      <div className="command-composer-line">
        <span className="command-composer-prompt" aria-hidden="true">#</span>
        <input
          ref={nlRef}
          className="command-composer-nl"
          value={state.nl}
          placeholder="例如：看一下电池温度和健康度"
          disabled={blocked || busy}
          onChange={(event) => dispatch({ type: "nl", value: event.target.value })}
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              event.preventDefault();
              onClose();
              return;
            }
            if (event.key === "Enter" && !event.nativeEvent.isComposing) {
              event.preventDefault();
              void run();
            }
          }}
        />
        <Button
          variant="secondary"
          disabled={blocked || busy || !state.nl.trim()}
          onClick={() => void run()}
          title="把需求交给模型生成命令草稿"
        >
          {busy ? <Spinner size={12} /> : "生成"}
        </Button>
      </div>

      {state.phase === "error" && state.error && (
        <div className="command-composer-error" role="alert">
          <AlertCircle size={14} />
          <span>{state.error}</span>
        </div>
      )}

      {state.phase === "ready" && (
        <>
          <div className="command-composer-line">
            <CornerDownLeft size={14} className="command-composer-draft-icon" aria-hidden="true" />
            <input
              ref={draftRef}
              className="command-composer-draft"
              value={state.editable}
              spellCheck={false}
              onChange={(event) => dispatch({ type: "edit", value: event.target.value })}
              onKeyDown={(event) => {
                if (event.key === "Escape") {
                  event.preventDefault();
                  onClose();
                  return;
                }
                if (event.key === "Tab") {
                  // Tab 是"接受并编辑"：插进输入框但不执行。
                  event.preventDefault();
                  insertOnly();
                  return;
                }
                if (event.key === "Enter" && !event.nativeEvent.isComposing) {
                  event.preventDefault();
                  execute();
                }
              }}
            />
            <Button
              type="button" variant="bare"
              className="command-composer-risk"
              data-risk={state.risk}
              title={
                state.riskEscalated
                  ? "模型低估了风险，已按本地规则升级"
                  : "命令风险等级"
              }
            >
              {state.risk !== "read-only" && <ShieldAlert size={12} />}
              {RISK_LABEL[state.risk]}
            </Button>
          </div>

          {state.explanation && <div className="command-composer-explain">{state.explanation}</div>}
          {state.needsRoot && (
            <div className="command-composer-note">
              <ShieldAlert size={13} />
              <span>这条命令需要 root 权限，设备未 root 时会失败。</span>
            </div>
          )}

          {needsAttention(state) && (
            <div className="command-composer-confirm">
              {state.risk === "destructive" ? (
                <>
                  <span>
                    风险为破坏性，请手打 <code>{expectedToken}</code> 才能执行：
                  </span>
                  <input
                    className="command-composer-confirm-input"
                    value={state.typed}
                    spellCheck={false}
                    placeholder={expectedToken}
                    onChange={(event) => dispatch({ type: "typed", value: event.target.value })}
                  />
                  {gate.ok === false && <span className="command-composer-confirm-reason">{gate.reason}</span>}
                </>
              ) : (
                <>
                  <span>这条命令会改变设备或系统状态。</span>
                  <Button variant="secondary" onClick={() => dispatch({ type: "confirm" })}>
                    我已确认，允许执行
                  </Button>
                </>
              )}
            </div>
          )}

          {remainingCount(state) > 0 && (
            <div className="command-composer-more">
              还有 {remainingCount(state)} 条待确认命令，执行完这条后逐条继续。
              <Button variant="bare" onClick={insertAll} title="把剩余命令一次性插入输入框，不执行">
                全部插入
              </Button>
            </div>
          )}

          <div className="command-composer-actions">
            <Button
              variant="secondary"
              icon={<Sparkles size={13} />}
              disabled={!gate.ok}
              onClick={execute}
              title={gate.ok ? "写入终端并回车" : gate.reason}
            >
              执行
            </Button>
            <Button variant="bare" onClick={insertOnly} title="插入到终端输入框，需你自己回车">
              只插入
            </Button>
          </div>
        </>
      )}

      <footer className="command-composer-hint" data-hint={gate.ok ? "ok" : "blocked"}>
        {composerHint(state)}
      </footer>
    </div>
  );
};

export default CommandComposer;
