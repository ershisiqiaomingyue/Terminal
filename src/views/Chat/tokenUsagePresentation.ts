import { formatTokenCount } from "./tokenCount.ts";

/**
 * Two different questions share one slot above the composer:
 *
 * 1. "What did this reply cost?" - this turn's generated tokens, the number
 *    that moves once per turn.
 * 2. "What has this whole conversation cost?" - session totals across every
 *    turn, which is the number people use to sanity-check a long session.
 *
 * Both are shown. The headline stays the turn's generated tokens (a session
 * input total sums every request and would swamp the line), while the session
 * totals keep the plain "输入 / 输出" pair they always had, now reached by
 * hovering the first item instead of eating three slots of the status bar.
 *
 * Input and output are never added together without being labelled: one is
 * re-sent context, the other is produced text.
 */
export interface TokenDisplay {
  /** This turn's counters. */
  input?: number;
  output?: number;
  reasoning?: number;
  /** Whole-session counters, already merged with the live turn by the caller. */
  session?: SessionTokenTotals;
}

export interface SessionTokenTotals {
  input?: number | null;
  output?: number | null;
  reasoning?: number | null;
  inputPartial?: boolean;
  outputPartial?: boolean;
  reasoningPartial?: boolean;
}

export interface TokenDetails {
  /** Generated tokens of this turn - the headline. */
  total?: number;
  /** Hover card body. */
  tokens: string;
}

function exact(value: number): string {
  return Math.max(0, Math.round(value)).toLocaleString("zh-CN");
}

/** Negative or non-finite readings are not measurements; treat them as absent. */
function sane(value: number | null | undefined): number | undefined {
  if (value == null || !Number.isFinite(value) || value < 0) return undefined;
  return Math.round(value);
}

/** "1,234+" when some turns never reported, so a total is a lower bound. */
function maybePartial(value: number | undefined, partial: boolean): string {
  if (value == null) return "未上报";
  return partial ? `${exact(value)}+` : exact(value);
}

export function turnTokenDetails(display: TokenDisplay): TokenDetails {
  const input = sane(display.input);
  const output = sane(display.output);
  const reasoning = sane(display.reasoning);
  const session = display.session;
  const sessionInput = sane(session?.input);
  const sessionOutput = sane(session?.output);
  const sessionReasoning = sane(session?.reasoning);

  const lines: string[] = [];
  if (output == null) {
    lines.push("本轮生成：供应商尚未上报");
  } else {
    lines.push(`本轮生成：${exact(output)} tokens`);
    // Reasoning is billed inside the output count, so it is a breakdown line
    // and never another term of the total.
    if (reasoning != null) lines.push(`其中思考 ${exact(reasoning)}（含在输出内）`);
  }
  if (input != null) {
    lines.push(`本轮请求输入累计：${exact(input)} tokens`);
    if (output != null && input > output * 10) {
      lines.push("输入远大于生成，是 agent 每轮重发上下文所致，不是写出来的字数。");
    }
  }

  lines.push("");
  if (sessionInput == null && sessionOutput == null) {
    lines.push("会话总计：尚未统计");
  } else {
    lines.push(`会话总计：输入 ${maybePartial(sessionInput, Boolean(session?.inputPartial))} · 输出 ${maybePartial(sessionOutput, Boolean(session?.outputPartial))} tokens`);
    if (sessionReasoning != null) lines.push(`会话思考：${maybePartial(sessionReasoning, Boolean(session?.reasoningPartial))} tokens（含在输出内）`);
    if (session?.inputPartial || session?.outputPartial) lines.push("带 + 的数值表示有对话轮次未上报，实际只会更多。");
  }
  return { total: output, tokens: lines.join("\n") };
}

/** Session input/output pair for the compact slot beside the headline. */
export function sessionTokenPair(session: SessionTokenTotals | undefined): { input: string; output: string } | undefined {
  if (!session) return undefined;
  const input = sane(session.input);
  const output = sane(session.output);
  if (input == null && output == null) return undefined;
  return {
    input: input == null ? "—" : formatTokenCount(input) + (session.inputPartial ? "+" : ""),
    output: output == null ? "—" : formatTokenCount(output) + (session.outputPartial ? "+" : ""),
  };
}

/** Headline for the item; compact because it sits on a 10px status bar. */
export function tokenHeadline(details: TokenDetails): string {
  return formatTokenCount(details.total);
}
