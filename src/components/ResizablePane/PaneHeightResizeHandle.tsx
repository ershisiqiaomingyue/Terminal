import { useEffect, useRef, useState, type PointerEvent } from "react";
import { clampPaneWidth } from "./paneWidth";

interface PaneHeightResizeHandleProps {
  label: string;
  controls: string;
  height: number;
  min: number;
  max: number;
  initial: number;
  onResize: (height: number) => void;
  onCommit: () => void;
  className?: string;
}

/**
 * 横向分隔条（纵向拖拽）：与 `PaneResizeHandle` 同一套指针捕获/节流/键盘语义，
 * 只把 `clientX` 换成 `clientY`。用于贴底面板的上沿——往上拖变大，往下拖变小。
 */
export function PaneHeightResizeHandle({ label, controls, height, min, max, initial, onResize, onCommit, className = "pane-height-resizer" }: PaneHeightResizeHandleProps) {
  const [dragging, setDragging] = useState(false);
  const drag = useRef<{ pointerId: number; y: number; height: number; latest: number } | null>(null);
  const frame = useRef(0);
  useEffect(() => () => { if (frame.current) cancelAnimationFrame(frame.current); }, []);
  const finish = (event: PointerEvent<HTMLDivElement>, restore = false) => {
    const current = drag.current;
    if (!current || event.pointerId !== current.pointerId) return;
    if (frame.current) cancelAnimationFrame(frame.current);
    frame.current = 0;
    onResize(restore ? current.height : current.latest);
    onCommit();
    drag.current = null;
    setDragging(false);
    if (event.currentTarget.hasPointerCapture(current.pointerId)) event.currentTarget.releasePointerCapture(current.pointerId);
  };

  return <div className={`${className}${dragging ? " is-dragging" : ""}`} role="separator" aria-label={label} title={label} aria-orientation="horizontal" aria-controls={controls} aria-valuemin={min} aria-valuemax={max} aria-valuenow={Math.round(height)} tabIndex={0}
    onPointerDown={(event) => {
      if (event.button !== 0 || drag.current) return;
      event.preventDefault();
      event.currentTarget.focus();
      event.currentTarget.setPointerCapture(event.pointerId);
      drag.current = { pointerId: event.pointerId, y: event.clientY, height, latest: height };
      setDragging(true);
    }}
    onPointerMove={(event) => {
      const current = drag.current;
      if (!current || current.pointerId !== event.pointerId) return;
      // 手柄在面板上沿：指针向上（clientY 变小）时面板变高。
      current.latest = clampPaneWidth(current.height - (event.clientY - current.y), { min, max });
      if (!frame.current) frame.current = requestAnimationFrame(() => { frame.current = 0; if (drag.current) onResize(drag.current.latest); });
    }}
    onPointerUp={finish} onPointerCancel={finish} onLostPointerCapture={finish}
    onDoubleClick={() => { onResize(clampPaneWidth(initial, { min, max })); onCommit(); }}
    onKeyDown={(event) => {
      const next = event.key === "ArrowUp" ? height + 10 : event.key === "ArrowDown" ? height - 10 : event.key === "Home" ? min : event.key === "End" ? max : null;
      if (next === null) return;
      event.preventDefault(); onResize(clampPaneWidth(next, { min, max })); onCommit();
    }} />;
}
