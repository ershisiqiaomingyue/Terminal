export interface DragPoint { x: number; y: number }
export interface DragBounds { left: number; right: number; top: number; bottom: number }
export function clampTabDragPreview(point: DragPoint, size: { width: number; height: number }, viewport: { width: number; height: number }) {
  const clampAxis = (position: number, length: number, available: number) => {
    const limit = Math.max(0, available - length);
    const inset = Math.min(8, limit / 2);
    return Math.max(inset, Math.min(position, limit - inset));
  };
  return { x: clampAxis(point.x, size.width, viewport.width), y: clampAxis(point.y, size.height, viewport.height) };
}

export function shouldDetachTab(start: DragPoint, current: DragPoint, bounds: DragBounds) {
  return Math.hypot(current.x - start.x, current.y - start.y) >= 12
    && (current.x < bounds.left - 16 || current.x > bounds.right + 16 || current.y < bounds.top - 16 || current.y > bounds.bottom + 16);
}
