/**
 * 命令块 AI 面板（开发方案 9.2.4 / 任务 T3-4）。
 *
 * 为什么是"底部停靠面板"而不是"输出行之间插入卡片"：
 * 终端输出由内核 canvas 直接绘制（`TerminalPane.tsx` 的 host 是 canvas），
 * 行与行之间没有 React 节点可插，插入 DOM 会立刻被内核重绘覆盖。所以卡片
 * 只能浮在终端视图之上——焦点面板底部，按块 id 打开，同一时刻只显示一个。
 *
 * 职责边界：本文件只管画和收集用户动作，所有请求编排都在
 * `terminalAiService.ts`，所有状态迁移都在 `blockAiPanel.ts`（纯函数、可单测）。
 */
import React, { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";
import { AlertCircle, Check, Copy, HelpCircle, Stethoscope, Terminal, Wrench, X } from "lucide-react";
import { Button, Spinner } from "@/components/ui";
import { MarkdownMessage } from "@/views/Chat/MarkdownMessage";
import type { CommandBlock } from "@/platform/terminal/blocks/types.ts";
import { runBlockAi, type BlockAiOutcome, type TerminalModelSelection } from "@/platform/terminal/ai/terminalAiService.ts";
import { terminalAiDeps } from "@/platform/terminal/ai/terminalAiDeps.ts";
import {
  blockAiReducer,
  extractSuggestedCommands,
  formatBlockAiMeta,
  initialBlockAiState,
  isBusy,
  summarizeBlockAi,
  type BlockAiCardState,
} from "./blockAiState.ts";
import type { TerminalAiMode } from "@/platform/terminal/ai/prompts.ts";

export interface BlockAiPanelProps {
  block: CommandBlock;
  /** 打开面板时的默认动作；失败块默认给"诊断"。 */
  initialMode?: TerminalAiMode;
  /** 使用的模型；未给则由服务层自动挑选第一个可用供应商。 */
  selection?: TerminalModelSelection;
  /** 关闭面板（不取消已发出的请求时由内部 abort）。 */
  onClose: () => void;
  /** 建议命令一键插入到输入面板；**只插入不执行**（F44）。 */
  onInsertCommand?: (command: string) => void;
  /**
   * 诊断完成后请求"尝试修复"（9.4 / T4-7）：把诊断原文交给父级去生成修复命令。
   * 父级负责审批卡片与执行，本组件不碰 PTY。
   */
  onRepairRequest?: (diagnosis: string) => void;
}

export const BlockAiPanel: React.FC<BlockAiPanelProps> = ({
  block,
  initialMode = "explain",
  selection,
  onClose,
  onInsertCommand,
  onRepairRequest,
}) => {
  const [state, dispatch] = useReducer(blockAiReducer, block.id, (id) => initialBlockAiState(id, initialMode));
  const [copied, setCopied] = useState(false);
  const [question, setQuestion] = useState("");
  const abortRef = useRef<AbortController | null>(null);
  // 每次请求自增，回调里比对，避免"旧请求的流式增量"写进新请求的状态。
  const runIdRef = useRef(0);
  const mountedRef = useRef(true);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      abortRef.current?.abort();
    };
  }, []);

  const run = useCallback(
    async (mode: TerminalAiMode, userQuestion?: string) => {
      abortRef.current?.abort();
      const controller = new AbortController();
      abortRef.current = controller;
      const runId = ++runIdRef.current;
      dispatch({ type: "start", blockId: block.id, mode });

      let outcome: BlockAiOutcome;
      try {
        outcome = await runBlockAi(block, mode, terminalAiDeps(), {
          selection,
          question: userQuestion,
          signal: controller.signal,
          // 流式增量：只接受本次请求的增量。
          onDelta: (chunk) => {
            if (runIdRef.current !== runId || !mountedRef.current) return;
            dispatch({ type: "delta", text: chunk });
          },
        });
      } catch (reason) {
        // `runBlockAi` 本身不抛异常，这里是最后的兜底，避免面板卡在 loading。
        outcome = {
          ok: false,
          mode,
          code: "invoke_failed",
          message: reason instanceof Error ? reason.message : String(reason),
        };
      }
      if (runIdRef.current !== runId || !mountedRef.current) return;

      if (outcome.ok) {
        dispatch({ type: "success", outcome });
        if (userQuestion) {
          dispatch({ type: "followUp", question: userQuestion, answer: outcome.text });
        }
      } else {
        dispatch({ type: "failure", outcome });
      }
    },
    [block, selection],
  );

  // 打开即请求一次：用户在块上点了"解释"，不该再点第二次。
  useEffect(() => {
    void run(initialMode);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [block.id, initialMode]);

  const busy = isBusy(state);
  const summary = useMemo(() => summarizeBlockAi(state.text), [state.text]);
  const suggestions = useMemo(
    () => (state.mode === "diagnose" && state.status === "done" ? extractSuggestedCommands(state.text) : []),
    [state.mode, state.status, state.text],
  );

  const copy = () => {
    void navigator.clipboard?.writeText(state.text).then(
      () => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1600);
      },
      () => undefined,
    );
  };

  const submitFollowUp = () => {
    const text = question.trim();
    if (!text || busy) return;
    setQuestion("");
    void run(state.mode, text);
  };

  const title = state.mode === "diagnose" ? "诊断这条命令" : "解释这条命令";

  return (
    <div className="block-ai-panel" data-status={state.status} role="region" aria-label={title}>
      <header className="block-ai-head">
        <div className="block-ai-title">
          {state.mode === "diagnose" ? <Stethoscope size={14} /> : <HelpCircle size={14} />}
          <span>{title}</span>
          <code className="block-ai-cmd" title={block.command}>
            {block.command.length > 48 ? `${block.command.slice(0, 47)}…` : block.command}
          </code>
          {block.exitCode !== undefined && (
            <span className="block-ai-exit" data-ok={block.exitCode === 0 ? "true" : "false"}>
              exit {block.exitCode}
            </span>
          )}
        </div>
        <div className="block-ai-head-actions">
          {/* 两个动作互斥：已经解释过就建议诊断，反之亦然。 */}
          <Button
            variant="bare"
            size="sm"
            disabled={busy}
            onClick={() => void run(state.mode === "explain" ? "diagnose" : "explain")}
            title={state.mode === "explain" ? "改问：为什么失败 / 怎么修" : "改问：这条命令在做什么"}
          >
            {state.mode === "explain" ? "诊断" : "解释"}
          </Button>
          <Button variant="bare" size="sm" icon={<X size={14} />} aria-label="关闭" title="关闭" onClick={onClose} />
        </div>
      </header>

      <div className="block-ai-body">
        {state.status === "loading" && !state.text && (
          <div className="block-ai-placeholder">
            <Spinner size={13} />
            <span>正在读取这条命令的输出…</span>
          </div>
        )}

        {state.error && (
          <div className="block-ai-error" role="alert">
            <AlertCircle size={14} />
            <div>
              <div className="block-ai-error-text">{state.error.message}</div>
              <div className="block-ai-error-code">错误码：{state.error.code}</div>
            </div>
            <Button variant="subtle" size="xs" onClick={() => void run(state.mode)} disabled={busy}>
              重试
            </Button>
          </div>
        )}

        {/* 追问历史：只保留最近 2 轮（`MAX_FOLLOW_UP_ROUNDS`）。 */}
        {state.followUps.map((item, index) => (
          <div className="block-ai-followup" key={`${index}-${item.question}`}>
            <div className="block-ai-question">
              <span>追问</span>
              {item.question}
            </div>
            <MarkdownMessage content={item.answer} />
          </div>
        ))}

        {state.text ? (
          <div className="block-ai-answer">
            <MarkdownMessage content={state.text} streaming={state.status === "streaming"} />
            {state.status === "streaming" && <span className="block-ai-caret" aria-hidden="true" />}
          </div>
        ) : null}

        {suggestions.length > 0 && (
          <div className="block-ai-suggestions">
            <div className="block-ai-suggestions-title">
              <Terminal size={13} />
              <span>建议命令</span>
              <em>仅插入到输入框，不会自动执行</em>
            </div>
            {suggestions.map((command) => (
              <div className="block-ai-suggestion" key={command}>
                <code>{command}</code>
                <Button
                  variant="subtle"
                  size="xs"
                  disabled={!onInsertCommand}
                  onClick={() => onInsertCommand?.(command)}
                  title="插入到终端输入框，需你确认后再回车"
                >
                  插入
                </Button>
              </div>
            ))}
          </div>
        )}
      </div>

      <footer className="block-ai-foot">
        <div className="block-ai-meta" title={summary}>
          {state.status === "done" ? formatBlockAiMeta(state.meta) : ""}
        </div>
        <div className="block-ai-foot-actions">
          {onRepairRequest && state.mode === "diagnose" && state.status === "done" && state.text ? (
            <Button
              variant="secondary"
              size="sm"
              icon={<Wrench size={13} />}
              onClick={() => onRepairRequest(state.text)}
              title="按这份诊断生成一条修复命令；生成后仍需你审批才会执行"
            >
              尝试修复
            </Button>
          ) : null}
          <Button
            variant="bare"
            size="sm"
            icon={copied ? <Check size={13} /> : <Copy size={13} />}
            disabled={!state.text}
            onClick={copy}
            title="复制回答"
          >
            {copied ? "已复制" : "复制"}
          </Button>
        </div>
      </footer>

      {state.status === "done" && (
        <div className="block-ai-ask">
          <input
            className="block-ai-ask-input"
            value={question}
            placeholder="继续追问，例如「那第二条怎么改」"
            onChange={(event) => setQuestion(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && !event.nativeEvent.isComposing) submitFollowUp();
            }}
            disabled={busy}
          />
          <Button variant="secondary" size="sm" disabled={busy || !question.trim()} onClick={submitFollowUp}>
            追问
          </Button>
        </div>
      )}
    </div>
  );
};

export default BlockAiPanel;
