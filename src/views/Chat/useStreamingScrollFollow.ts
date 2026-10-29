import { type KeyboardEvent, type RefObject, type UIEvent, type WheelEvent, useCallback, useLayoutEffect, useRef, useState } from "react";
import { followStreamingAfterScroll, scrollOwnerConsumesDelta } from "./conversationScrollState";

/**
 * 让一个自带滚动区的面板在流式输出时贴着底部走，读者往回翻就停在原地。
 *
 * 三件事各管一段：
 * - 内容变长（`revision` 变化，或内部高度被 ResizeObserver 观测到增长）
 *   且仍在跟随 → 贴到底部；
 * - 读者往上滚、拖滚动条、或用键盘回溯 → 停止跟随；
 * - 读者滚回底部附近 → 自动重新跟随，不用额外点按钮。
 *
 * 「是谁在滚」不靠「高度变了就是我滚的」判断：内容增长时高度先变、贴底在后，
 * 两者之间没有可观察的时序差；比较 scrollTop 的方向就能把向上的读者移动与
 * 向下的程序贴底区分开。
 *
 * 滚轮与键盘额外走手势入口（`onWheel` / `onKeyDown`）：这两类意图在手势那一刻
 * 就能判定，不用等 scroll 事件派发——中间若正好赶上下一个流式增量，贴底会把
 * 读者刚滚到的位置直接抹掉。滚动条拖拽没有手势入口，只能靠 scroll 采样，
 * 但拖拽是连续动作，漏掉一次采样也会被下一次补上。
 *
 * 跟随状态同时存在 ref 与 state 里：ref 供事件同步读取（一次渲染里可能连来
 * 好几个），state 只为让「已暂停」这个事实能被渲染出来。翻转很少发生，
 * 不会造成流式期间的重渲染风暴。
 *
 * ResizeObserver 是给「尺寸晚于文本到达」的内容准备的：折叠代码块、公式、
 * 图表挂载后都会再长一截，只靠 `revision` 会停在那之前的位置。
 */
export function useStreamingScrollFollow(
  revision: string,
  container: RefObject<HTMLElement | null>,
  startFollowing: boolean,
  armed = true,
) {
  // useRef 只取首次渲染的值：展开一个已经结束的思考时不该跳到末尾。
  const followingRef = useRef(startFollowing);
  const [following, setFollowing] = useState(startFollowing);
  const lastTopRef = useRef(0);

  const mark = useCallback((next: boolean) => {
    followingRef.current = next;
    // 同值 setState 会被 React 直接丢弃，不必自己判重。
    setFollowing(next);
  }, []);

  const stick = useCallback(() => {
    const node = container.current;
    if (!node || !followingRef.current) return;
    node.scrollTop = node.scrollHeight;
    lastTopRef.current = node.scrollTop;
  }, [container]);

  // 每次「展开」都从干净状态起算。折叠期间滚动区整个不在 DOM 里，读者留在
  // 半空的 scrollTop 已无意义；而上一次「已经接管」的记忆若留在 ref 里，
  // 重新展开一段仍在生成、或已经结束的思考就都不会跟到底了。
  const startRef = useRef(startFollowing);
  startRef.current = startFollowing;
  const armedRef = useRef(armed);
  useLayoutEffect(() => {
    if (armed && !armedRef.current) {
      mark(startRef.current);
      lastTopRef.current = 0;
      stick();
    }
    armedRef.current = armed;
  }, [armed, mark, stick]);

  // 内容每变一次就贴一次底。用 layout effect 是为了在绘制前完成，
  // 否则流式增量会让视口先闪一帧「新内容在下方看不见」。
  useLayoutEffect(() => {
    stick();
  }, [revision, stick]);

  // 内容块的高度变化同样要贴底（图表、折叠代码、图片解码后都会再长一截）。
  // 观察器只在挂载时建一次 —— 依赖里放 revision 会让每个 token 都重建一个。
  const stickRef = useRef(stick);
  stickRef.current = stick;
  useLayoutEffect(() => {
    const node = container.current;
    const content = node?.firstElementChild;
    if (!node || !content || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => stickRef.current());
    observer.observe(content);
    return () => observer.disconnect();
  }, [container]);

  const onScroll = useCallback((event: UIEvent<HTMLElement>) => {
    const node = event.currentTarget;
    const next = followStreamingAfterScroll(
      followingRef.current,
      node.scrollHeight,
      node.scrollTop,
      lastTopRef.current,
      node.clientHeight,
    );
    lastTopRef.current = node.scrollTop;
    if (next !== followingRef.current) mark(next);
  }, [mark]);

  // 滚轮往上 = 读者要看更早的内容。往下滚不取消跟随：滚到底部时 onScroll
  // 会按距离把跟随接回来，提前判真只会在半路把视点拽走。
  // 先问一句「这次增量是不是真被卡片吃掉了」：卡片还没溢出、或已经停在顶部时，
  // 滚轮其实是要滚外面的对话，此时把卡片停半空是误伤。
  const onWheel = useCallback((event: WheelEvent<HTMLElement>) => {
    if (event.deltaY >= 0 || !followingRef.current) return;
    const node = event.currentTarget;
    if (!scrollOwnerConsumesDelta(node.scrollHeight, node.clientHeight, node.scrollTop, event.deltaY)) return;
    mark(false);
  }, [mark]);

  const onKeyDown = useCallback((event: KeyboardEvent<HTMLElement>) => {
    if (!followingRef.current) return;
    if (event.key !== "ArrowUp" && event.key !== "PageUp" && event.key !== "Home") return;
    const node = event.currentTarget;
    if (node.scrollHeight - node.clientHeight <= 1 || node.scrollTop <= 1) return;
    mark(false);
  }, [mark]);

  // 「回到最新」：滚回底部本来就是天然入口，但键盘用户翻下来之后不一定正好
  // 停在阈值内，给一颗明确的按钮比猜读者意图可靠。
  const resume = useCallback(() => {
    mark(true);
    stick();
  }, [mark, stick]);

  return { following, onKeyDown, onScroll, onWheel, resume };
}
