import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { ChevronDown, LoaderCircle, PanelTop, Plus, X } from "lucide-react";
import { createPortal } from "react-dom";
import type React from "react";
import { partitionTabs } from "./tabOverflow";
import { subscribeAppFont } from "@/platform/font";
import { useTabDetachDrag } from "./useTabDetachDrag";
import { clampTabDragPreview } from "./tabDrag";

interface WindowTab {
  id: string;
  title: string;
  closable: boolean;
  icon?: React.ReactNode;
}

interface TopbarTabsProps {
  tabs: readonly WindowTab[];
  activeId: string;
  onSelect: (id: string) => void;
  onPromote?: (id: string) => void;
  onClose: (id: string) => void;
  onAdd?: () => void;
  onDetach?: (id: string, point: { x: number; y: number }) => void | Promise<void>;
  canDetach?: (id: string) => boolean;
}

export function TopbarTabs({ tabs, activeId, onSelect, onPromote, onClose, onAdd, onDetach, canDetach }: TopbarTabsProps) {
  const rootRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const measureRef = useRef<HTMLDivElement>(null);
  const previewRef = useRef<HTMLDivElement>(null);
  const [dimensions, setDimensions] = useState({ width: 0, widths: new Map<string, number>() });
  const { visible, hidden } = partitionTabs(tabs, activeId, dimensions.width, dimensions.widths, { maxVisible: 5, gap: 3, moreWidth: 28 });
  const overflow = hidden.length > 0;
  const [open, setOpen] = useState(false);
  const [menuPosition, setMenuPosition] = useState({ top: 0, left: 0, width: 208 });
  const menuId = useId();
  const showMenu = open && overflow;
  const { preview, opening, beginDrag, moveDrag, endDrag, clearDrag, selectTab } = useTabDetachDrag(rootRef, onSelect, onDetach, canDetach);
  const previewTab = preview && tabs.find((tab) => tab.id === preview.id);

  useLayoutEffect(() => {
    const element = previewRef.current;
    if (!preview || !element) return;
    // Clamp only the visual preview; the original pointer coordinates determine the drop.
    const position = () => {
      const point = clampTabDragPreview({ x: preview.left, y: preview.top },
        { width: element.offsetWidth, height: element.offsetHeight },
        { width: document.documentElement.clientWidth, height: document.documentElement.clientHeight });
      element.style.transform = `translate3d(${point.x}px, ${point.y}px, 0)`;
    };
    position();
    const observer = new ResizeObserver(position);
    observer.observe(element);
    window.addEventListener("resize", position);
    return () => { observer.disconnect(); window.removeEventListener("resize", position); };
  }, [preview]);

  const measure = useCallback(() => {
    const root = rootRef.current;
    const measurement = measureRef.current;
    if (!root || !measurement) return;
    const width = Math.max(0, root.clientWidth - (onAdd ? 36 : 5));
    const widths = new Map(Array.from(measurement.children, (child) => [(child as HTMLElement).dataset.tabId!, child.getBoundingClientRect().width]));
    setDimensions((previous) => previous.width === width && previous.widths.size === widths.size && Array.from(widths).every(([id, value]) => previous.widths.get(id) === value) ? previous : { width, widths });
  }, [onAdd]);

  useLayoutEffect(measure, [measure, tabs]);
  useLayoutEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const observer = new ResizeObserver(measure);
    observer.observe(root);
    if (measureRef.current) observer.observe(measureRef.current);
    let disposed = false;
    void document.fonts.ready.then(() => { if (!disposed) measure(); });
    // 换界面字体后每个标签的宽度都变了，旧的量测结果会让省略/溢出判断错位。
    const unsubscribeFont = subscribeAppFont(() => {
      void document.fonts.ready.then(() => { if (!disposed) measure(); });
    });
    return () => { disposed = true; observer.disconnect(); unsubscribeFont(); };
  }, [measure]);

  useLayoutEffect(() => {
    setOpen(false);
  }, [activeId, tabs.length]);

  useLayoutEffect(() => {
    if (!showMenu || !triggerRef.current) return;
    const rect = triggerRef.current.getBoundingClientRect();
    const width = Math.min(208, window.innerWidth - 16);
    setMenuPosition({ top: rect.bottom + 4, left: Math.max(8, Math.min(rect.right - width, window.innerWidth - width - 8)), width });
  }, [showMenu]);

  useEffect(() => {
    if (!showMenu) return;
    menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitemradio"]')?.focus();
    const outside = (event: PointerEvent) => { if (!rootRef.current?.contains(event.target as Node)) setOpen(false); };
    const resize = () => { setOpen(false); triggerRef.current?.focus(); };
    document.addEventListener("pointerdown", outside);
    window.addEventListener("resize", resize);
    return () => { document.removeEventListener("pointerdown", outside); window.removeEventListener("resize", resize); };
  }, [showMenu]);

  const focusActive = () => requestAnimationFrame(() => {
    (listRef.current?.querySelector<HTMLButtonElement>('[role="tab"][aria-selected="true"]') || listRef.current?.querySelector<HTMLButtonElement>('[role="tab"]'))?.focus();
  });
  const choose = (id: string) => { onSelect(id); setOpen(false); focusActive(); };
  const close = (id: string) => { onClose(id); setOpen(false); focusActive(); };

  return <div ref={rootRef} className="topbar-tabs">
    <div ref={measureRef} className="topbar-tabs-measure" aria-hidden="true">
      {tabs.map((tab) => <div key={tab.id} data-tab-id={tab.id} className="topbar-tab">
        <span className="topbar-tab-select">{tab.icon && <span className="topbar-tab-icon">{tab.icon}</span>}<span className="topbar-tab-title">{tab.title}</span></span>
        {tab.closable && <span className="topbar-tab-close"><X size={12} /></span>}
      </div>)}
    </div>
    <div ref={listRef} className="topbar-tab-list" role="tablist" aria-label="已打开的工具" onKeyDown={(event) => {
      const tab = (event.target as HTMLElement).closest<HTMLButtonElement>('[role="tab"]');
      if (!tab) return;
      const index = visible.findIndex((item) => item.id === tab.dataset.tabId);
      let next: number | undefined;
      if (event.key === "Home") next = 0;
      else if (event.key === "End") next = visible.length - 1;
      else if (event.key === "ArrowRight") next = (index + 1) % visible.length;
      else if (event.key === "ArrowLeft") next = (index - 1 + visible.length) % visible.length;
      else if (event.key === "Delete" && visible[index]?.closable) { event.preventDefault(); close(visible[index].id); }
      if (next !== undefined) { event.preventDefault(); choose(visible[next].id); }
    }}>
      {visible.map((tab, index) => <div key={tab.id} className={`topbar-tab${tab.id === activeId ? " is-active" : ""}${preview?.id === tab.id ? " is-dragging-out" : ""}`} role="presentation" data-tauri-drag-region="false">
        <button type="button" className="topbar-tab-select" data-tab-id={tab.id} role="tab" aria-selected={tab.id === activeId} aria-busy={opening === tab.id} tabIndex={tab.id === activeId || (!tabs.some((item) => item.id === activeId) && index === 0) ? 0 : -1} title={opening === tab.id ? "正在打开独立窗口…" : tab.title} onPointerDown={(event) => beginDrag(event, tab.id)} onPointerMove={moveDrag} onPointerUp={endDrag} onPointerCancel={clearDrag} onLostPointerCapture={clearDrag} onClick={() => selectTab(tab.id)}>{(tab.icon || opening === tab.id) && <span className="topbar-tab-icon" aria-hidden="true">{opening === tab.id ? <LoaderCircle className="topbar-tab-opening" size={14} /> : tab.icon}</span>}<span className="topbar-tab-title">{tab.title}</span></button>
        {tab.closable && <button type="button" className="topbar-tab-close" title={`关闭${tab.title}`} aria-label={`关闭${tab.title}`} disabled={opening === tab.id} onClick={() => close(tab.id)}><X size={12} /></button>}
      </div>)}
    </div>
    <div className="topbar-tab-actions" data-tauri-drag-region="false">
    {overflow && <div className="topbar-tabs-overflow" data-tauri-drag-region="false">
      <button ref={triggerRef} type="button" className="topbar-tabs-more" title="已打开的工具" aria-label="已打开的工具" aria-haspopup="menu" aria-expanded={showMenu} aria-controls={showMenu ? menuId : undefined} onClick={() => setOpen((value) => !value)} onKeyDown={(event) => { if (event.key === "ArrowDown" || event.key === "ArrowUp") { event.preventDefault(); setOpen(true); } }}><ChevronDown size={16} /></button>
      {showMenu && <div ref={menuRef} id={menuId} className="topbar-tabs-menu" style={menuPosition} data-tauri-drag-region="false" role="menu" aria-label="已打开的工具" onKeyDown={(event) => {
        if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); setOpen(false); triggerRef.current?.focus(); return; }
        if (event.key === "Tab") { setOpen(false); return; }
        const buttons = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('button'));
        const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
        let next: number | undefined;
        if (event.key === "Home") next = 0;
        else if (event.key === "End") next = buttons.length - 1;
        else if (event.key === "ArrowDown") next = (index + 1) % buttons.length;
        else if (event.key === "ArrowUp") next = (index - 1 + buttons.length) % buttons.length;
        if (next !== undefined) { event.preventDefault(); buttons[next]?.focus(); }
      }}>
        {hidden.map((tab) => <div key={tab.id} className={`topbar-tabs-menu-row${tab.id === activeId ? " is-active" : ""}`} role="none">
          <button type="button" className="topbar-tabs-menu-select" role="menuitemradio" aria-checked={tab.id === activeId} title={tab.title} onClick={() => { onPromote?.(tab.id); choose(tab.id); }}>{tab.icon && <span className="topbar-tab-icon" aria-hidden="true">{tab.icon}</span>}<span className="topbar-tab-title">{tab.title}</span></button>
          {tab.closable && <button type="button" className="topbar-tab-close" role="menuitem" title={`关闭${tab.title}`} aria-label={`关闭${tab.title}`} onClick={() => close(tab.id)}><X size={12} /></button>}
        </div>)}
      </div>}
      </div>}
      {onAdd && <button type="button" className="topbar-tab-add" title="打开工具" aria-label="打开工具" onClick={onAdd}><Plus size={16} /></button>}
    </div>
    {preview && previewTab && createPortal(<div ref={previewRef} className={`topbar-tab-drag-preview${preview.canRelease ? " is-ready" : ""}`} style={{ minWidth: `min(${preview.width}px, calc(100vw - 16px))` }} aria-hidden="true">
      <div className="topbar-tab-drag-card">
        <div className="topbar-tab-drag-title">{previewTab.icon && <span className="topbar-tab-icon">{previewTab.icon}</span>}<span className="topbar-tab-title">{previewTab.title}</span><PanelTop size={14} /></div>
        <span className="topbar-tab-drag-hint">{preview.canRelease ? "松开以在新窗口中打开" : "拖出标签栏以打开新窗口"}</span>
      </div>
    </div>, document.body)}
  </div>;
}
