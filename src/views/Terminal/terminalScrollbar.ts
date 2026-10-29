/**
 * 终端自绘滚动条（DOM 版，替代内核内建的那条）。
 *
 * 为什么不用内核内建：自研内核把滚动条**画进 canvas 自己的宽高里**
 * （ghostty-web `renderer.ts:1069` `scrollbarX = canvasWidth - 8 - 4`），而 canvas 只有
 * `cols × 字宽` 宽、比宿主窄一截。于是滚动条缩在画布右缘、脱离面板右框，还会在正文
 * 字符上方压出一条 8px 轨道底色；面板右边再空出一条 92px 的空白（`terminalFit.ts`
 * 又额外 `cols - 1` 预留了一次）。
 *
 * 这里改成真 DOM：位置交给 CSS（贴面板右缘），几何只吃内核给的
 * `rows / baseY / viewportY`，换算全在 `src/platform/terminalScroll.ts` 的纯函数里。
 * 用命令式更新而不是 React state：滚动时每帧都要改 `transform`，走 state 会让整个
 * 面板每帧重渲染。
 */
import type { AppTerminal } from "@/platform/terminalAdapter";
import { SCROLLBAR_TRACK_PADDING, scrollbarGeometry, scrollLinesFromPointer } from "@/platform/terminalScroll";

/** 拖动时两次 pointermove 之间的换算基准（用起始几何，避免累积误差）。 */
interface DragState {
  pointerId: number;
  thumbHeight: number;
  trackHeight: number;
  appliedPointerY: number;
}

export interface TerminalScrollbarHandle {
  /** 重新量尺寸并重画；尺寸/设置变化时由面板调用。 */
  update(): void;
  dispose(): void;
}

/**
 * 在 `host` 的父级（`.terminal-output-viewport`）里挂一条滚动条。
 *
 * `host` 是 `.terminal-xterm-fit`，它的高度就是可见文本区高度，滚动条轨道与之等高。
 * `isEnabled` 每帧现读（设置项「内建滚动条」开关），关掉时整条 `display: none`。
 */
export function mountTerminalScrollbar(
  term: AppTerminal,
  host: HTMLElement,
  isEnabled: () => boolean,
): TerminalScrollbarHandle {
  const viewport = host.parentElement;
  if (!viewport) {
    return { update: () => undefined, dispose: () => undefined };
  }

  const root = document.createElement("div");
  root.className = "terminal-scrollbar";
  root.setAttribute("aria-hidden", "true");
  const thumb = document.createElement("div");
  thumb.className = "terminal-scrollbar-thumb";
  root.append(thumb);
  viewport.append(root);

  let drag: DragState | null = null;
  let hiddenTimer: number | null = null;

  const active = () => term.buffer.active;

  /**
   * 重画一次。`data-scrollable` 也在这里写：回滚内容是在写入过程中长出来的，
   * 若只在 update() 里写，滚动回调触发时开关仍停在旧值、滚动条永远不出现。
   */
  const paint = () => {
    const { thumbTop, thumbHeight, visible } = scrollbarGeometry({
      rows: term.rows,
      baseY: active().baseY,
      viewportY: active().viewportY,
      trackHeight: host.clientHeight,
    });
    if (!visible || !isEnabled()) {
      root.dataset.scrollable = "false";
      return;
    }
    root.dataset.scrollable = "true";
    thumb.style.height = `${thumbHeight}px`;
    thumb.style.transform = `translateY(${thumbTop}px)`;
  };

  /** 滚动时闪现一下再淡出，避免静止时干扰阅读。 */
  const flash = () => {
    root.dataset.active = "true";
    if (hiddenTimer !== null) window.clearTimeout(hiddenTimer);
    hiddenTimer = window.setTimeout(() => {
      hiddenTimer = null;
      if (!drag) root.dataset.active = "false";
    }, 1200);
  };

  const update = () => paint();

  const scrollToPointer = (clientY: number, state: DragState) => {
    const rect = root.getBoundingClientRect();
    const pointerTop = clientY - rect.top;
    const delta = pointerTop - state.thumbHeight / 2 - state.appliedPointerY;
    if (Math.abs(delta) < 0.5) return;
    state.appliedPointerY += delta;
    const lines = scrollLinesFromPointer({
      deltaY: delta,
      trackHeight: state.trackHeight,
      baseY: active().baseY,
      thumbHeight: state.thumbHeight,
    });
    if (lines) term.scrollLines(lines);
  };

  const onThumbPointerDown = (event: PointerEvent) => {
    if (event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    const geometry = scrollbarGeometry({
      rows: term.rows,
      baseY: active().baseY,
      viewportY: active().viewportY,
      trackHeight: host.clientHeight,
    });
    if (!geometry.visible) return;
    // `thumbTop` 已经是滑块的起始位置，指针按在滑块中间 ⇒ 基准就是 thumbTop。
    drag = {
      pointerId: event.pointerId,
      thumbHeight: geometry.thumbHeight,
      trackHeight: host.clientHeight,
      appliedPointerY: geometry.thumbTop,
    };
    root.dataset.dragging = "true";
    thumb.setPointerCapture(event.pointerId);
  };

  const onPointerMove = (event: PointerEvent) => {
    if (!drag || event.pointerId !== drag.pointerId) return;
    event.preventDefault();
    scrollToPointer(event.clientY, drag);
  };

  const endDrag = (event: PointerEvent) => {
    if (!drag || event.pointerId !== drag.pointerId) return;
    drag = null;
    delete root.dataset.dragging;
    if (thumb.hasPointerCapture(event.pointerId)) thumb.releasePointerCapture(event.pointerId);
    flash();
  };

  /** 点轨道空白：滑块中心跳到指针处，再按拖动同一套换算滚过去。 */
  const onTrackPointerDown = (event: PointerEvent) => {
    if (event.button !== 0 || event.target === thumb) return;
    event.preventDefault();
    const geometry = scrollbarGeometry({
      rows: term.rows,
      baseY: active().baseY,
      viewportY: active().viewportY,
      trackHeight: host.clientHeight,
    });
    if (!geometry.visible) return;
    const rect = root.getBoundingClientRect();
    const maxTop = Math.max(
      SCROLLBAR_TRACK_PADDING,
      host.clientHeight - SCROLLBAR_TRACK_PADDING - geometry.thumbHeight,
    );
    const targetTop = Math.min(Math.max(SCROLLBAR_TRACK_PADDING, event.clientY - rect.top - geometry.thumbHeight / 2), maxTop);
    // 与拖动共用同一个换算，避免两套公式在极端尺寸下互相打架。
    const lines = scrollLinesFromPointer({
      deltaY: targetTop - geometry.thumbTop,
      trackHeight: host.clientHeight,
      baseY: active().baseY,
      thumbHeight: geometry.thumbHeight,
    });
    if (lines) term.scrollLines(lines);
    flash();
  };

  const scrollSubscription = term.onScroll(() => {
    paint();
    flash();
  });
  const observer = new ResizeObserver(() => update());

  thumb.addEventListener("pointerdown", onThumbPointerDown);
  thumb.addEventListener("pointermove", onPointerMove);
  thumb.addEventListener("pointerup", endDrag);
  thumb.addEventListener("pointercancel", endDrag);
  root.addEventListener("pointerdown", onTrackPointerDown);
  observer.observe(host);
  update();

  return {
    update,
    dispose: () => {
      if (hiddenTimer !== null) window.clearTimeout(hiddenTimer);
      observer.disconnect();
      scrollSubscription.dispose();
      thumb.removeEventListener("pointerdown", onThumbPointerDown);
      thumb.removeEventListener("pointermove", onPointerMove);
      thumb.removeEventListener("pointerup", endDrag);
      thumb.removeEventListener("pointercancel", endDrag);
      root.removeEventListener("pointerdown", onTrackPointerDown);
      root.remove();
    },
  };
}
