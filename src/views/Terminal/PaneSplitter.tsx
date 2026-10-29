import React, { useCallback, useRef, useState } from "react";

import { clampPaneRatio, paneRatioFromPointer } from "./paneLayout";

/**
 * 面板分隔条（需求 ⑥：拖边线缩放窗口）。
 *
 * ## 为什么拖动过程中**不**走 React state
 *
 * 分隔条每次 pointermove 都会算出一个新比例。如果每一帧 `setState`，整个
 * `TerminalView`（1700 行、持有终端实例池与四个面板）就要重渲染一次——
 * 那正是用户三令五申不能接受的性能开销。
 *
 * 所以这里的分工是：
 * - **拖动中**：只往网格元素上写一个 CSS 变量（`--pane-col-ratio` / `--pane-row-ratio`），
 *   浏览器自己做布局，React 一次都不渲染；
 * - **松手时**：才把最终比例交给父级（父级据此决定是普通缩放还是越阈值抢占）。
 *
 * 这也是不给网格加 `transition` 的原因：拖拽要求跟手，补间会显得发黏。
 */

/** 键盘微调步长：0.02 约等于一栏宽度的 2%，够细也不至于按十几次看不出变化。 */
const KEYBOARD_STEP = 0.02;

export interface PaneSplitterProps {
  /** `col` = 左右两块之间的竖条；`row` = 上下两块之间的横条。 */
  axis: "col" | "row";
  /** 当前比例（父级上次提交的值），键盘微调与 ARIA 数值都基于它。 */
  ratio: number;
  /** 拖动中的实时比例。父级应**只写 DOM**，不要在这里 setState。 */
  onPreview: (ratio: number) => void;
  /** 松手 / 键盘调整后的最终比例。父级在这里才允许动 state 或弹确认。 */
  onCommit: (ratio: number) => void;
}

interface DragState {
  pointerId: number;
  startRatio: number;
  /** 可分配尺寸（已扣掉 gap）。用于把像素位移换算成比例。 */
  axisSize: number;
  startX: number;
  startY: number;
}

export const PaneSplitter: React.FC<PaneSplitterProps> = ({ axis, ratio, onPreview, onCommit }) => {
  const dragRef = useRef<DragState | null>(null);
  const [dragging, setDragging] = useState(false);

  /**
   * 可分配尺寸 = 网格**内容盒**在该轴上的长度 − gap。
   *
   * 必须扣掉内边距和 gap：网格有 `padding: 8px` 与 `gap: 8px`，而
   * `getBoundingClientRect()` 给的是含内边距的外框。用外框换算会让「拖到中线」
   * 常态化偏十几像素，越拖越歪。口径要和 CSS 里
   * `calc((100% - 24px) * var(--pane-col-ratio))` 完全一致。
   */
  const measure = useCallback((grid: HTMLElement, which: "col" | "row"): number => {
    const style = window.getComputedStyle(grid);
    const padStart = which === "col" ? style.paddingLeft : style.paddingTop;
    const padEnd = which === "col" ? style.paddingRight : style.paddingBottom;
    const rawGap = which === "col" ? style.columnGap : style.rowGap;
    const gap = Number.parseFloat(rawGap) || 0;
    const pad = (Number.parseFloat(padStart) || 0) + (Number.parseFloat(padEnd) || 0);
    const outer = which === "col" ? grid.clientWidth : grid.clientHeight;
    return Math.max(0, outer - pad - gap);
  }, []);

  const handlePointerDown = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      // 只认主键：右键/中键按下时不该开始拖拽。
      if (event.button !== 0) return;
      const grid = event.currentTarget.parentElement;
      if (!grid) return;
      const axisSize = measure(grid, axis);
      if (axisSize <= 0) return;
      event.preventDefault();
      event.stopPropagation();
      event.currentTarget.setPointerCapture(event.pointerId);
      dragRef.current = {
        pointerId: event.pointerId,
        startRatio: clampPaneRatio(ratio),
        axisSize,
        startX: event.clientX,
        startY: event.clientY,
      };
      setDragging(true);
    },
    [axis, measure, ratio],
  );

  const handlePointerMove = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      const drag = dragRef.current;
      if (!drag || drag.pointerId !== event.pointerId) return;
      // 位移**始终相对起点**计算，不在上一帧结果上累加：夹取到边界后不会积攒误差。
      const delta = axis === "col" ? event.clientX - drag.startX : event.clientY - drag.startY;
      onPreview(paneRatioFromPointer(drag.startRatio, delta, drag.axisSize));
    },
    [axis, onPreview],
  );

  const finish = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      const drag = dragRef.current;
      if (!drag || drag.pointerId !== event.pointerId) return;
      dragRef.current = null;
      setDragging(false);
      if (event.currentTarget.hasPointerCapture(event.pointerId)) {
        event.currentTarget.releasePointerCapture(event.pointerId);
      }
      const delta = axis === "col" ? event.clientX - drag.startX : event.clientY - drag.startY;
      onCommit(paneRatioFromPointer(drag.startRatio, delta, drag.axisSize));
    },
    [axis, onCommit],
  );

  const handleKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      const back = axis === "col" ? "ArrowLeft" : "ArrowUp";
      const forward = axis === "col" ? "ArrowRight" : "ArrowDown";
      if (event.key !== back && event.key !== forward) return;
      event.preventDefault();
      const next = clampPaneRatio(ratio + (event.key === forward ? KEYBOARD_STEP : -KEYBOARD_STEP));
      // 键盘是离散操作，直接预览 + 提交，不需要中间的拖动状态。
      onPreview(next);
      onCommit(next);
    },
    [axis, onCommit, onPreview, ratio],
  );

  const percent = Math.round(clampPaneRatio(ratio) * 1000) / 10;

  return (
    <div
      className="terminal-pane-splitter"
      data-axis={axis}
      data-dragging={dragging ? "true" : "false"}
      role="separator"
      tabIndex={0}
      aria-orientation={axis === "col" ? "vertical" : "horizontal"}
      aria-label={axis === "col" ? "调整左右面板宽度" : "调整上下面板高度"}
      aria-valuenow={percent}
      aria-valuemin={0}
      aria-valuemax={100}
      title={axis === "col" ? "拖动调整左右宽度，拖到底可让一侧独占" : "拖动调整上下高度，拖到底可让一侧独占"}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={finish}
      onPointerCancel={finish}
      onKeyDown={handleKeyDown}
      // 分隔条上的右键菜单/长按拖拽都不应该冒泡到面板，否则会误开终端菜单。
      onContextMenu={(event) => event.preventDefault()}
    />
  );
};
