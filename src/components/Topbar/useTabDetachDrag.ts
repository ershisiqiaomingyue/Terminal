import { useCallback, useEffect, useRef, useState, type PointerEvent } from "react";
import { shouldDetachTab } from "./tabDrag";

interface DragSession {
  id: string;
  x: number;
  y: number;
  offsetX: number;
  offsetY: number;
  width: number;
  pointer: number;
  moved: boolean;
  target: HTMLButtonElement;
}

export interface TabDragPreview {
  id: string;
  left: number;
  top: number;
  width: number;
  canRelease: boolean;
}

export function useTabDetachDrag(
  rootRef: { current: HTMLDivElement | null },
  onSelect: (id: string) => void,
  onDetach?: (id: string, point: { x: number; y: number }) => void | Promise<void>,
  canDetach?: (id: string) => boolean,
) {
  const drag = useRef<DragSession | null>(null);
  const suppressClick = useRef<string | null>(null);
  const frame = useRef<number | null>(null);
  const nextPreview = useRef<TabDragPreview | null>(null);
  const [preview, setPreview] = useState<TabDragPreview | null>(null);
  const [opening, setOpening] = useState<string | null>(null);

  const clearDrag = useCallback(() => {
    const current = drag.current;
    drag.current = null;
    if (frame.current !== null) cancelAnimationFrame(frame.current);
    frame.current = null;
    nextPreview.current = null;
    setPreview(null);
    if (current?.moved) suppressClick.current = current.id;
    if (current?.target.hasPointerCapture(current.pointer)) current.target.releasePointerCapture(current.pointer);
  }, []);

  useEffect(() => {
    if (!preview) return;
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") { event.preventDefault(); clearDrag(); }
    };
    document.body.classList.add("is-detaching-tab");
    window.addEventListener("keydown", escape);
    window.addEventListener("blur", clearDrag);
    return () => {
      document.body.classList.remove("is-detaching-tab");
      window.removeEventListener("keydown", escape);
      window.removeEventListener("blur", clearDrag);
    };
  }, [!!preview, clearDrag]);

  useEffect(() => () => {
    if (frame.current !== null) cancelAnimationFrame(frame.current);
    const current = drag.current;
    drag.current = null;
    if (current?.target.hasPointerCapture(current.pointer)) current.target.releasePointerCapture(current.pointer);
  }, []);

  const beginDrag = (event: PointerEvent<HTMLButtonElement>, id: string) => {
    if (event.button !== 0 || opening || !onDetach || !canDetach?.(id)) return;
    const rect = event.currentTarget.parentElement!.getBoundingClientRect();
    suppressClick.current = null;
    event.currentTarget.setPointerCapture(event.pointerId);
    drag.current = {
      id, x: event.clientX, y: event.clientY, offsetX: event.clientX - rect.left,
      offsetY: event.clientY - rect.top, width: rect.width, pointer: event.pointerId,
      moved: false, target: event.currentTarget,
    };
    onSelect(id);
  };

  const moveDrag = (event: PointerEvent<HTMLButtonElement>) => {
    const current = drag.current;
    const bounds = rootRef.current?.getBoundingClientRect();
    if (!current || current.pointer !== event.pointerId || !bounds) return;
    if (!current.moved && Math.hypot(event.clientX - current.x, event.clientY - current.y) < 12) return;
    current.moved = true;
    nextPreview.current = {
      id: current.id, left: event.clientX - current.offsetX, top: event.clientY - current.offsetY,
      width: current.width, canRelease: shouldDetachTab(current, { x: event.clientX, y: event.clientY }, bounds),
    };
    if (frame.current === null) frame.current = requestAnimationFrame(() => {
      frame.current = null;
      setPreview(nextPreview.current);
    });
  };

  const endDrag = (event: PointerEvent<HTMLButtonElement>) => {
    const current = drag.current;
    if (!current || current.pointer !== event.pointerId) return;
    const bounds = rootRef.current?.getBoundingClientRect();
    clearDrag();
    if (!bounds || !shouldDetachTab(current, { x: event.clientX, y: event.clientY }, bounds)) return;
    const point = { x: event.screenX * window.devicePixelRatio, y: event.screenY * window.devicePixelRatio };
    setOpening(current.id);
    void Promise.resolve().then(() => onDetach?.(current.id, point))
      .catch((error) => console.error("打开独立工具窗口失败", error)).finally(() => setOpening(null));
  };

  const selectTab = (id: string) => {
    if (suppressClick.current === id) { suppressClick.current = null; return; }
    onSelect(id);
  };
  return { preview, opening, beginDrag, moveDrag, endDrag, clearDrag, selectTab };
}
