/**
 * 标题滚动（marquee）：照抄 dsh 的 `useTitleMarquee` 行为。
 *
 * 行高只有 32px，标题必须单行省略；一旦悬停就把被截断的尾巴以恒定速度爬出来，
 * 爬到尾部停住（不是来回循环），指针离开后一步回到起点——回程也做成动画会和
 * 静止态的省略号打架。溢出不足 8px 时干脆不动，那种抖动读起来像掉帧。
 */
import { useEffect, useMemo, useRef, type RefObject } from "react";

/** 这么小的溢出藏不住有意义的尾巴，滚起来只会像抖动。 */
export const MIN_TITLE_REVEAL_PX = 8;

/** 爬动速度：慢到能边滚边读。 */
export const TITLE_MARQUEE_PX_PER_MS = 0.03;

/**
 * 写入标题的滚动位置，并维护两端的渐隐开关：`data-scrolled` 表示已经离开起点
 * （左端渐隐），`data-clipped` 表示右侧还有字（右端渐隐）。
 */
export function placeTitle(title: HTMLSpanElement, left: number, range: number): void {
  if (typeof title.scrollTo === "function") title.scrollTo({ left, behavior: "instant" });
  else title.scrollLeft = left;
  if (left > 0) title.dataset.scrolled = "";
  else delete title.dataset.scrolled;
  if (left < range) title.dataset.clipped = "";
  else delete title.dataset.clipped;
}

/** 回到静息态：位置归零、两道渐隐都关掉，静止的省略号以全强度显示。 */
export function restTitle(title: HTMLSpanElement): void {
  if (typeof title.scrollTo === "function") title.scrollTo({ left: 0, behavior: "instant" });
  else title.scrollLeft = 0;
  delete title.dataset.scrolled;
  delete title.dataset.clipped;
}

/**
 * 悬停时把超出格子的标题爬到尾部。
 * @param title 行内负责裁剪的标题元素。
 * @returns 稳定的进入/离开处理器，挂到整行上。
 */
export function useTitleMarquee(title: RefObject<HTMLSpanElement | null>): { enter: () => void; leave: () => void } {
  const frame = useRef(0);
  useEffect(() => () => { cancelAnimationFrame(frame.current); }, []);
  return useMemo(() => ({
    enter: () => {
      const element = title.current;
      if (!element) return;
      cancelAnimationFrame(frame.current);
      let previous: number | undefined;
      let position = 0;
      let range = -1;
      const step = (now: number) => {
        // 行程要等悬停后的布局定下来再量：悬停会换掉行尾的「时间+图钉」，
        // 标题可用宽度随之变化，用进入瞬间的宽度算出来的行程会差一截。
        if (range < 0) {
          range = element.scrollWidth - element.clientWidth;
          if (range <= MIN_TITLE_REVEAL_PX) return;
          if (window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) {
            placeTitle(element, range, range);
            return;
          }
        }
        position += previous === undefined ? 0 : (now - previous) * TITLE_MARQUEE_PX_PER_MS;
        previous = now;
        placeTitle(element, Math.min(position, range), range);
        if (position < range) frame.current = requestAnimationFrame(step);
      };
      frame.current = requestAnimationFrame(step);
    },
    leave: () => {
      cancelAnimationFrame(frame.current);
      const element = title.current;
      if (element) restTitle(element);
    },
  }), [title]);
}
