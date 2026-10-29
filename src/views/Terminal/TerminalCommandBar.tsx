/**
 * 终端底部指令发送栏（设计稿 `design-drafts/terminal/index.html` 的 `.terminal-card-footer`）。
 *
 * 两行结构：
 * 1. AI 智能行：自然语言 → 交给输入侧 AI 面板（不在这里直接发请求）；
 * 2. CLI 执行行：手输命令 → 父级写 PTY 并回车。
 *
 * 为什么把 AI 行做成"只转交不执行"：
 * 自然语言到命令的中间有草稿确认与风险闸门（9.3 / 10.1），
 * 若在这一行直接回车执行，用户会跳过草稿编辑与 `mutating`/`destructive` 确认。
 */
import React, { useCallback, useEffect, useRef, useState } from "react";
import { CornerDownLeft, Sparkles } from "lucide-react";

export interface TerminalCommandBarProps {
  /** 当前页签目标类型，决定 CLI 行的占位提示。 */
  target: "adb" | "ssh" | "wsl" | "local";
  /** 直通模式（全屏程序运行中）下 AI 行必须禁用（9.3.4）。 */
  aiBlockedReason?: string | null;
  /** 会话尚未建立时禁用整个底栏。 */
  cliBlockedReason?: string | null;
  /** CLI 行回车：父级写 PTY 并执行。 */
  onSubmitCommand: (command: string) => void;
  /** AI 行回车：带着自然语言打开输入侧面板。 */
  onAskAi: (naturalLanguage: string) => void;
}

const CLI_PLACEHOLDER: Record<TerminalCommandBarProps["target"], string> = {
  local: "输入命令并按回车执行…",
  wsl: "输入 WSL 命令并按回车…",
  adb: "输入 adb shell 命令并按回车…",
  ssh: "输入 SSH 远程命令并按回车…",
};

export const TerminalCommandBar: React.FC<TerminalCommandBarProps> = ({
  target,
  aiBlockedReason,
  cliBlockedReason,
  onSubmitCommand,
  onAskAi,
}) => {
  const [cli, setCli] = useState("");
  const [nl, setNl] = useState("");
  /** 本次会话内已提交的命令，用于 ↑/↓ 翻。不放 localStorage：终端重开即清空更符合直觉。 */
  const [history, setHistory] = useState<string[]>([]);
  const cursorRef = useRef(-1);
  const cliRef = useRef<HTMLInputElement | null>(null);

  // 换页签即清空草稿：避免把上一台设备的命令带到另一台上。
  useEffect(() => {
    setCli("");
    setNl("");
    setHistory([]);
    cursorRef.current = -1;
  }, [target]);

  const submitCommand = useCallback(() => {
    const command = cli.trim();
    if (!command || cliBlockedReason) return;
    onSubmitCommand(command);
    // 相邻重复不重复入栈，连按回车不至于把历史塞满。
    setHistory((prev) => (prev[prev.length - 1] === command ? prev : [...prev, command]));
    cursorRef.current = -1;
    setCli("");
    // 保持焦点：连续发下一条不用再点一次。
    window.requestAnimationFrame(() => cliRef.current?.focus());
  }, [cli, cliBlockedReason, onSubmitCommand]);

  const askAi = useCallback(() => {
    const text = nl.trim();
    if (!text || aiBlockedReason) return;
    setNl("");
    onAskAi(text);
  }, [aiBlockedReason, nl, onAskAi]);

  const recallHistory = (delta: 1 | -1) => {
    if (!history.length) return;
    const base = cursorRef.current;
    const next = base < 0 ? (delta < 0 ? history.length - 1 : -1) : base + delta;
    if (next < 0 || next >= history.length) {
      cursorRef.current = -1;
      setCli("");
      return;
    }
    cursorRef.current = next;
    setCli(history[next]);
  };

  const onCliKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter") {
      // 输入法组字中回车只是选词，不能当提交。
      if (event.nativeEvent.isComposing) return;
      event.preventDefault();
      submitCommand();
      return;
    }
    if (event.key === "ArrowUp") {
      event.preventDefault();
      recallHistory(-1);
      return;
    }
    if (event.key === "ArrowDown") {
      event.preventDefault();
      recallHistory(1);
    }
  };

  return (
    <div className="terminal-card-footer">
      <div className="ai-smart-line" data-blocked={aiBlockedReason ? "true" : "false"}>
        <Sparkles size={12} className="ai-smart-icon" aria-hidden />
        <input
          className="ai-smart-input"
          value={nl}
          onChange={(event) => setNl(event.target.value)}
          onKeyDown={(event) => {
            if (event.key !== "Enter" || event.nativeEvent.isComposing) return;
            event.preventDefault();
            askAi();
          }}
          placeholder={aiBlockedReason ?? "输入自然语言，由 AI 生成终端命令…"}
          disabled={Boolean(aiBlockedReason)}
          aria-label="用自然语言生成命令"
        />
        <button
          type="button"
          className="ai-smart-send"
          onClick={askAi}
          disabled={Boolean(aiBlockedReason) || !nl.trim()}
          title="生成命令草稿"
          aria-label="生成命令草稿"
        >
          <CornerDownLeft size={12} />
        </button>
      </div>

      <div className="cli-exec-line" data-blocked={cliBlockedReason ? "true" : "false"}>
        <span className="cli-arrow" aria-hidden>
          ❯
        </span>
        <input
          ref={cliRef}
          className="cli-input"
          value={cli}
          onChange={(event) => {
            setCli(event.target.value);
            cursorRef.current = -1;
          }}
          onKeyDown={onCliKeyDown}
          placeholder={cliBlockedReason ?? CLI_PLACEHOLDER[target]}
          disabled={Boolean(cliBlockedReason)}
          spellCheck={false}
          autoComplete="off"
          aria-label="输入命令"
        />
      </div>
    </div>
  );
};

export default TerminalCommandBar;
