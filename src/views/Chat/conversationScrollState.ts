export const FOLLOW_BOTTOM_THRESHOLD = 80;
export const USER_SCROLL_DELTA_THRESHOLD = 0.5;

export function distanceFromBottom(scrollHeight: number, scrollTop: number, clientHeight: number) {
  return Math.max(0, scrollHeight - scrollTop - clientHeight);
}

export function isAwayFromBottom(scrollHeight: number, scrollTop: number, clientHeight: number) {
  return distanceFromBottom(scrollHeight, scrollTop, clientHeight) > FOLLOW_BOTTOM_THRESHOLD;
}

export function shouldScheduleAutomaticFollow(following: boolean, userInteractionPending: boolean) {
  return following && !userInteractionPending;
}

/**
 * 一个可滚动元素能否消费这次滚轮的增量。
 *
 * 用于区分「滚轮滚的是对话本身」和「滚轮滚的是对话里某个自带滚动区的面板」
 * （执行过程详情、实时输出、长代码块等）。后者不该被当成读者在往回翻对话，
 * 否则在内层面板里看一眼日志就会把整个对话的自动跟随暂停掉。
 */
export function scrollOwnerConsumesDelta(
  scrollHeight: number,
  clientHeight: number,
  scrollTop: number,
  deltaY: number,
) {
  const overflow = scrollHeight - clientHeight;
  if (overflow <= 1) return false;
  return deltaY < 0 ? scrollTop > 1 : scrollTop < overflow - 1;
}

export function hasMeaningfulScrollDelta(scrollDelta: number) {
  return Math.abs(scrollDelta) > USER_SCROLL_DELTA_THRESHOLD;
}

/**
 * 展开的思考卡片：内容流式增长时贴着底部走，读者一旦往回翻就停在原处。
 *
 * 阈值比对话整体（80px）小得多 —— 卡片只有 320px 高，留 80px 余量会让
 * 「往回滚一点」也算还在底部，滚动条刚拖动一点就被重新拉回去。
 */
export const STREAMING_FOLLOW_THRESHOLD = 24;

export function followsStreamingTail(scrollHeight: number, scrollTop: number, clientHeight: number) {
  return distanceFromBottom(scrollHeight, scrollTop, clientHeight) <= STREAMING_FOLLOW_THRESHOLD;
}

/**
 * 采样一次卡片滚动，决定下一段增量还要不要继续跟随。
 *
 * 往上挪过（scrollTop 变小）就直接判定读者接管，不再看离底部多远：拖滚动条和
 * 滚轮最终都只是改变 scrollTop，而程序自己贴底是往下走，两者不会混淆。
 * 读者把内容滚回底部附近时重新跟随。
 */
export function followStreamingAfterScroll(
  following: boolean,
  scrollHeight: number,
  scrollTop: number,
  previousScrollTop: number,
  clientHeight: number,
) {
  if (scrollTop < previousScrollTop - USER_SCROLL_DELTA_THRESHOLD) return false;
  if (followsStreamingTail(scrollHeight, scrollTop, clientHeight)) return true;
  // 往下挪但还没到家：保持原状。这个分支也会兜住「程序刚贴底、scroll 事件
  // 派发前又长了一截」的竞态，不能因此误判成读者接管。
  return following;
}

export function automaticFollowAfterScroll(
  current: boolean,
  userIntent: boolean,
  awayFromBottom: boolean,
  scrollDelta: number,
  explicitPause = false,
) {
  if (!userIntent) return current;
  if (explicitPause && hasMeaningfulScrollDelta(scrollDelta)) return false;
  if (!awayFromBottom) return true;
  return scrollDelta < -USER_SCROLL_DELTA_THRESHOLD ? false : current;
}
