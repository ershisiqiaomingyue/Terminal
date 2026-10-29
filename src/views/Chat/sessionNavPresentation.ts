import type { ChatSession } from "./types";

/**
 * 侧栏会话行右侧的相对时间。
 *
 * 读者要靠它判断「这条对话是什么时候开的」，所以只给粗粒度档位：分钟以内不
 * 逐秒跳动，超过一个月就只说月份数——数字一直变会让人忍不住去盯它。
 */
export function formatSessionAge(createdAt: number, now: number): string {
  const elapsed = Math.max(0, now - createdAt);
  const minutes = Math.floor(elapsed / 60_000);
  if (minutes < 1) return "刚刚";
  // 中文按 dsh 的写法不带空格（「33分钟」而不是「33 分钟」）：数字与单位在中文里
  // 是一个词，插入空格会把 10px 的尾槽标签撑宽、逼得标题提前截断。
  if (minutes < 60) return `${minutes}分钟`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}小时`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}天`;
  const months = Math.floor(days / 30);
  if (months < 12) return `${months}个月`;
  return `${Math.floor(months / 12)}年`;
}

/**
 * 侧栏排序：置顶的永远在前，其余按最近活动倒序。
 *
 * 输入顺序来自历史库（最近活动的在前），但置顶是读者显式表达的意图，必须压过
 * 时间序；同一优先级内保持原有相对次序，避免列表在刷新时无故抖动。
 */
export function orderNavSessions(sessions: readonly ChatSession[]): ChatSession[] {
  return sessions
    .map((session, index) => ({ session, index }))
    .sort((left, right) => {
      const pinned = Number(Boolean(right.session.pinned)) - Number(Boolean(left.session.pinned));
      if (pinned !== 0) return pinned;
      const updated = (right.session.updatedAt || 0) - (left.session.updatedAt || 0);
      if (updated !== 0) return updated;
      return left.index - right.index;
    })
    .map((entry) => entry.session);
}
