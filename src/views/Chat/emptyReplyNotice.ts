/**
 * 「模型没有返回内容」这条兜底提示的成立条件。
 *
 * 追加要求（steer）会把同一轮切成多段，而只有**最后一段**是 pending。中间段
 * 既可能没有正文、也没有工具事件，但整轮其实还在跑，正文也可能落在别的段里，
 * 所以不能只看「这一段为空」就判定模型没有返回内容。
 */
export function shouldShowEmptyReplyNotice(options: {
  /** 这一段自己是否还在流式输出。 */
  pending: boolean;
  /** 整轮是否还在跑（同一轮所有段的聚合状态）。 */
  turnPending: boolean;
  /** 这一段在本轮里的序号，从 0 开始。 */
  turnSegmentIndex?: number;
  /** 本轮一共被切成了几段。 */
  turnSegmentCount?: number;
  /** 这一段有没有解析出正文。 */
  hasAnswer: boolean;
  /** 这一段有没有可展示的工具/时间线内容。 */
  hasTools: boolean;
}) {
  if (options.pending || options.turnPending) return false;
  const count = options.turnSegmentCount || 1;
  if ((options.turnSegmentIndex ?? 0) < count - 1) return false;
  return !options.hasAnswer && !options.hasTools;
}
