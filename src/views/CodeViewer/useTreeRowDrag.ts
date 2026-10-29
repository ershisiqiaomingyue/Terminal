import { useCallback, useEffect, useRef, useState } from "react";
import type { BrowserEntry } from "./useCodeBrowser";
import { canMoveEntry } from "./treeDrag";

// Windows 上 Tauri 的拖放接管（wry 会 SetAllowExternalDrop(false) 并在窗口注册
// 自己的 IDropTarget）会让页面内的 HTML5 拖放事件根本发不出来，所以工程树的
// 移动操作改为 pointer 事件驱动：行为一致，且与"从资源管理器拖文件进来"共存。
const DRAG_THRESHOLD = 5;

type Options = {
  enabled: boolean;
  onMove: (entry: BrowserEntry, destinationDirectory: string) => void;
  onDragEntryChange?: (entry: BrowserEntry | null) => void;
};

export function useTreeRowDrag({ enabled, onMove, onDragEntryChange }: Options) {
  const [draggedEntry, setDraggedEntry] = useState<BrowserEntry | null>(null);
  const [moveTarget, setMoveTarget] = useState("");
  const [pointer, setPointer] = useState({ x: 0, y: 0 });
  const pending = useRef<{ entry: BrowserEntry; x: number; y: number } | null>(null);
  const draggedRef = useRef<BrowserEntry | null>(null);
  const targetRef = useRef("");
  const suppressClickUntil = useRef(0);
  const onMoveRef = useRef(onMove);
  onMoveRef.current = onMove;
  const onChangeRef = useRef(onDragEntryChange);
  onChangeRef.current = onDragEntryChange;

  const reset = useCallback(() => {
    pending.current = null;
    draggedRef.current = null;
    targetRef.current = "";
    setDraggedEntry(null);
    setMoveTarget("");
    document.body.classList.remove("code-tree-dragging");
    onChangeRef.current?.(null);
  }, []);

  const begin = useCallback((entry: BrowserEntry, event: React.PointerEvent<HTMLElement>) => {
    if (!enabled || event.button !== 0 || event.altKey || event.ctrlKey || event.metaKey) return;
    pending.current = { entry, x: event.clientX, y: event.clientY };
  }, [enabled]);

  useEffect(() => {
    if (!enabled) return;
    const directoryAt = (x: number, y: number) => {
      const element = document.elementFromPoint(x, y);
      const holder = element instanceof Element ? element.closest<HTMLElement>("[data-code-directory]") : null;
      return holder?.dataset.codeDirectory || "";
    };
    const onPointerMove = (event: PointerEvent) => {
      const start = pending.current;
      if (!start) return;
      if (!draggedRef.current) {
        if (Math.hypot(event.clientX - start.x, event.clientY - start.y) < DRAG_THRESHOLD) return;
        draggedRef.current = start.entry;
        setDraggedEntry(start.entry);
        document.body.classList.add("code-tree-dragging");
        onChangeRef.current?.(start.entry);
      }
      event.preventDefault();
      setPointer({ x: event.clientX, y: event.clientY });
      const directory = directoryAt(event.clientX, event.clientY);
      const usable = Boolean(directory) && canMoveEntry(start.entry, directory);
      const next = usable ? directory : "";
      if (next !== targetRef.current) {
        targetRef.current = next;
        setMoveTarget(next);
      }
    };
    const finish = (commit: boolean) => {
      const source = draggedRef.current;
      const destination = targetRef.current;
      if (source) suppressClickUntil.current = Date.now() + 250;
      reset();
      if (commit && source && destination && canMoveEntry(source, destination)) onMoveRef.current(source, destination);
    };
    const onPointerUp = () => { if (draggedRef.current || pending.current) finish(true); };
    const onPointerCancel = () => { if (draggedRef.current || pending.current) finish(false); };
    const onKeyDown = (event: KeyboardEvent) => { if (event.key === "Escape" && draggedRef.current) finish(false); };
    const onClickCapture = (event: MouseEvent) => {
      if (Date.now() < suppressClickUntil.current) {
        event.preventDefault();
        event.stopPropagation();
      }
    };
    window.addEventListener("pointermove", onPointerMove, true);
    window.addEventListener("pointerup", onPointerUp, true);
    window.addEventListener("pointercancel", onPointerCancel, true);
    window.addEventListener("keydown", onKeyDown, true);
    document.addEventListener("click", onClickCapture, true);
    return () => {
      window.removeEventListener("pointermove", onPointerMove, true);
      window.removeEventListener("pointerup", onPointerUp, true);
      window.removeEventListener("pointercancel", onPointerCancel, true);
      window.removeEventListener("keydown", onKeyDown, true);
      document.removeEventListener("click", onClickCapture, true);
      document.body.classList.remove("code-tree-dragging");
    };
  }, [enabled, reset]);

  useEffect(() => reset, [reset]);

  return { begin, draggedEntry, moveTarget, pointer, dragging: draggedEntry != null, cancel: reset };
}
