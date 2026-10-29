/**
 * 指针拖拽（v3）：左栏卡片 / 面板标题栏共用的拖拽内核。
 *
 * 为什么不用 HTML5 DnD：Tauri 在 Windows WebView2 上默认接管拖放
 * （`dragDropEnabled` 默认开，聊天/代码查看器依赖它接收文件拖入），原生处理器
 * 会吞掉页面内 dragover/drop——真机上「左栏拖到面板」完全无响应。改为指针
 * 事件自绘拖拽（阈值启动 + 命中测试 + dataset 高亮），与 WebView 行为解耦，
 * 浏览器模式下同样工作。
 *
 * 全程只写 dataset/DOM，不触发 React 渲染（终端视图重渲染代价高）。
 */

export interface PointerDragOptions {
  /** 拖影上显示的文字（通常是页签标题 / 服务器名）。 */
  label: string;
  /** 命中测试的元素选择器（默认 `.terminal-pane`）。 */
  targetSelector?: string;
  /** 拖动源自身的元素：命中它时不算有效目标（面板拖自己没有意义）。 */
  ignore?: HTMLElement | null;
  /** 松手在有效目标上；xRatio/yRatio 是相对目标内容盒的 0..1 坐标。 */
  onDrop: (target: HTMLElement, xRatio: number, yRatio: number) => void;
  /** 手势结束后（无论是否成拖）是否要吞掉紧随的 click（卡片自身有点击语义时用）。 */
  suppressClickAfter?: boolean;
}

const DRAG_THRESHOLD_PX = 5;

/**
 * 在 `onPointerDown` 里调用：移动超过 5px 才真正进入拖拽（否则仍是点击），
 * 拖影跟随指针、命中目标高亮 `data-drag-over`，松手回调 `onDrop`。
 * 不会抛异常（命中测试/清理都做了防御），可以放心直接接线。
 */
export function startPointerDrag(event: React.PointerEvent<HTMLElement>, options: PointerDragOptions): void {
  if (event.button !== 0) return;
  // 动作区（关闭 × / 迷你按钮 / 链接 / 输入框）有自己的语义，不进入拖拽。
  if ((event.target as HTMLElement).closest(".ui-list-item-actions, .terminal-session-close, .terminal-sidebar-mini-btn, button.terminal-pane-close, input, a")) return;
  const selector = options.targetSelector ?? ".terminal-pane";
  const source = event.currentTarget;
  const ignore = options.ignore ?? null;
  const startX = event.clientX;
  const startY = event.clientY;
  const previousSelect = document.body.style.userSelect;
  let ghost: HTMLDivElement | null = null;
  let hovered: HTMLElement | null = null;
  let dragging = false;
  let done = false;

  const cleanup = () => {
    window.removeEventListener("pointermove", onMove);
    if (ghost) ghost.remove();
    ghost = null;
    delete source.dataset.dragging;
    if (hovered) delete hovered.dataset.dragOver;
    hovered = null;
    document.body.style.userSelect = previousSelect;
    document.body.style.cursor = "";
  };

  const onMove = (move: PointerEvent) => {
    if (done) return;
    if (!dragging) {
      if (Math.hypot(move.clientX - startX, move.clientY - startY) < DRAG_THRESHOLD_PX) return;
      dragging = true;
      source.dataset.dragging = "true";
      document.body.style.userSelect = "none";
      document.body.style.cursor = "grabbing";
      ghost = document.createElement("div");
      ghost.className = "terminal-drag-ghost";
      ghost.textContent = options.label;
      document.body.appendChild(ghost);
    }
    if (ghost) {
      ghost.style.left = `${move.clientX + 12}px`;
      ghost.style.top = `${move.clientY + 10}px`;
    }
    const target = (document.elementFromPoint(move.clientX, move.clientY) as HTMLElement | null)
      ?.closest<HTMLElement>(selector) ?? null;
    const effective = target && (!ignore || !ignore.contains(target)) ? target : null;
    if (effective !== hovered) {
      if (hovered) delete hovered.dataset.dragOver;
      if (effective) effective.dataset.dragOver = "true";
      hovered = effective;
    }
  };

  const finish = (up: PointerEvent, cancelled: boolean) => {
    if (done) return;
    done = true;
    const target = hovered;
    const rect = target?.getBoundingClientRect();
    const wasDragging = dragging;
    cleanup();
    if (!wasDragging || cancelled || !target || !rect) return;
    if (options.suppressClickAfter) {
      // 拖完这次手势后吞掉紧随的 click，否则松手会顺手触发卡片自己的点击。
      source.addEventListener("click", (event0) => event0.stopPropagation(), { once: true, capture: true });
    }
    options.onDrop(
      target,
      rect.width > 0 ? (up.clientX - rect.left) / rect.width : Number.NaN,
      rect.height > 0 ? (up.clientY - rect.top) / rect.height : Number.NaN,
    );
  };

  window.addEventListener("pointermove", onMove);
  window.addEventListener("pointerup", (up) => finish(up, false), { once: true });
  window.addEventListener("pointercancel", (up) => finish(up, true), { once: true });
}
