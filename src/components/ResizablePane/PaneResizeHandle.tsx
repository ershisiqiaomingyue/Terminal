import { useEffect, useRef, useState, type PointerEvent } from "react";
import { clampPaneWidth } from "./paneWidth";

interface PaneResizeHandleProps {
  label: string;
  controls: string;
  width: number;
  min: number;
  max: number;
  initial: number;
  onResize: (width: number) => void;
  onCommit: () => void;
  side?: "left" | "right";
  className?: string;
}

export function PaneResizeHandle({ label, controls, width, min, max, initial, onResize, onCommit, side = "left", className = "code-viewer-pane-resizer" }: PaneResizeHandleProps) {
  const [dragging, setDragging] = useState(false);
  const drag = useRef<{ pointerId: number; x: number; width: number; latest: number } | null>(null);
  const frame = useRef(0);
  useEffect(() => () => { if (frame.current) cancelAnimationFrame(frame.current); }, []);
  const finish = (event: PointerEvent<HTMLDivElement>, restore = false) => {
    const current = drag.current;
    if (!current || event.pointerId !== current.pointerId) return;
    if (frame.current) cancelAnimationFrame(frame.current);
    frame.current = 0;
    onResize(restore ? current.width : current.latest);
    onCommit();
    drag.current = null;
    setDragging(false);
    if (event.currentTarget.hasPointerCapture(current.pointerId)) event.currentTarget.releasePointerCapture(current.pointerId);
  };

  const direction = side === "right" ? -1 : 1;
  return <div className={`${className}${dragging ? " is-dragging" : ""}`} role="separator" aria-label={label} title={label} aria-orientation="vertical" aria-controls={controls} aria-valuemin={min} aria-valuemax={max} aria-valuenow={Math.round(width)} tabIndex={0}
    onPointerDown={(event) => {
      if (event.button !== 0 || drag.current) return;
      event.preventDefault();
      event.currentTarget.focus();
      event.currentTarget.setPointerCapture(event.pointerId);
      drag.current = { pointerId: event.pointerId, x: event.clientX, width, latest: width };
      setDragging(true);
    }}
    onPointerMove={(event) => {
      const current = drag.current;
      if (!current || current.pointerId !== event.pointerId) return;
      current.latest = clampPaneWidth(current.width + (event.clientX - current.x) * direction, { min, max });
      if (!frame.current) frame.current = requestAnimationFrame(() => { frame.current = 0; if (drag.current) onResize(drag.current.latest); });
    }}
    onPointerUp={finish} onPointerCancel={finish} onLostPointerCapture={finish}
    onDoubleClick={() => { onResize(clampPaneWidth(initial, { min, max })); onCommit(); }}
    onKeyDown={(event) => {
      const next = event.key === "ArrowLeft" ? width - 10 * direction : event.key === "ArrowRight" ? width + 10 * direction : event.key === "Home" ? min : event.key === "End" ? max : null;
      if (next === null) return;
      event.preventDefault(); onResize(clampPaneWidth(next, { min, max })); onCommit();
    }} />;
}
