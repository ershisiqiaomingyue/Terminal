import { useCallback, useLayoutEffect, useRef, useState, type MouseEvent } from "react";
import { Button } from "rain-ui";
import { X } from "lucide-react";
import { partitionFileTabs, pathsToClose, type TabCloseAction } from "./fileTabs";
import { ContextMenu } from "rain-ui";
import { subscribeAppFont } from "@/platform/font";
import { ToolbarPopover } from "./ToolbarPopover";

export type FileTabItem = { path: string; title: string; dirty: boolean; readOnly?: boolean };

const TAB_GAP = 2;
const MORE_WIDTH = 26;

// 收起来的数量用三个点表示（竖排），固定贴在标签条最右侧。
function OverflowDots() {
  return <svg className="code-viewer-tab-more-icon" viewBox="0 0 1024 1024" aria-hidden="true" focusable="false">
    <path d="M512 533.333333m-85.333333 0a85.333333 85.333333 0 1 0 170.666666 0 85.333333 85.333333 0 1 0-170.666666 0Z" fill="currentColor" />
    <path d="M170.666667 533.333333m-85.333334 0a85.333333 85.333333 0 1 0 170.666667 0 85.333333 85.333333 0 1 0-170.666667 0Z" fill="currentColor" />
    <path d="M853.333333 533.333333m-85.333333 0a85.333333 85.333333 0 1 0 170.666667 0 85.333333 85.333333 0 1 0-170.666667 0Z" fill="currentColor" />
  </svg>;
}

// 第二行的文件标签条：按每个标签的实际宽度算出能摆下几个，摆不下的收进最右侧的三个点里。
// 标签标题只取文件名（重名时带父目录），有未保存改动就在名字右侧标 *。
export function FileTabStrip({ tabs, activePath, onSelect, onClose, onCloseMany, onSelectFromOverflow, label }: {
  tabs: readonly FileTabItem[];
  activePath: string;
  onSelect: (path: string) => void;
  onClose: (path: string) => void;
  // 右键菜单的批量关闭：一次关多个，未保存改动的确认由调用方一次性处理。
  onCloseMany?: (paths: readonly string[], action: TabCloseAction) => void;
  // 从悬浮层里选中一个文件：调用方会把它挪到标签条第一个。
  onSelectFromOverflow?: (path: string) => void;
  label: string;
}) {
  const rootRef = useRef<HTMLDivElement>(null);
  const measureRef = useRef<HTMLDivElement>(null);
  const moreRef = useRef<HTMLButtonElement>(null);
  const [width, setWidth] = useState(0);
  const [widths, setWidths] = useState<ReadonlyMap<string, number>>(new Map());
  const [overflowOpen, setOverflowOpen] = useState(false);
  const [menu, setMenu] = useState<{ path: string; x: number; y: number; trigger: HTMLElement } | null>(null);
  const openMenu = (path: string, event: MouseEvent<HTMLElement>) => {
    if (!onCloseMany) return;
    event.preventDefault();
    setMenu({ path, x: event.clientX, y: event.clientY, trigger: event.currentTarget });
  };

  const measure = useCallback(() => {
    const root = rootRef.current, probe = measureRef.current;
    if (!root || !probe) return;
    const next = new Map<string, number>();
    for (const child of Array.from(probe.children)) {
      const element = child as HTMLElement;
      const path = element.dataset.tabPath;
      if (path) next.set(path, element.getBoundingClientRect().width);
    }
    const nextWidth = root.clientWidth;
    setWidth((current) => (current === nextWidth ? current : nextWidth));
    setWidths((current) => {
      if (current.size === next.size && Array.from(next).every(([path, value]) => current.get(path) === value)) return current;
      return next;
    });
  }, []);

  useLayoutEffect(measure, [measure, tabs]);
  useLayoutEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const observer = new ResizeObserver(measure);
    observer.observe(root);
    if (measureRef.current) observer.observe(measureRef.current);
    let disposed = false;
    // 字体没就绪时量出来的宽度偏小，加载完成后补一次。
    void document.fonts.ready.then(() => { if (!disposed) measure(); });
    // 界面字体切换后标签宽度全部变化，量测结果必须跟着重算，否则收纳点会错位。
    const unsubscribeFont = subscribeAppFont(() => {
      void document.fonts.ready.then(() => { if (!disposed) measure(); });
    });
    return () => { disposed = true; observer.disconnect(); unsubscribeFont(); };
  }, [measure]);
  // 标签全部关掉时悬浮层不该继续挂着。
  useLayoutEffect(() => { if (!tabs.length && overflowOpen) setOverflowOpen(false); }, [tabs.length, overflowOpen]);

  const { visible, hidden, flexible } = partitionFileTabs(tabs, activePath, width, widths, { gap: TAB_GAP, moreWidth: MORE_WIDTH });
  const measurements = <div ref={measureRef} className="code-viewer-tab-strip-measure" aria-hidden="true">
    {tabs.map((tab) => <div key={tab.path} data-tab-path={tab.path} className={`code-viewer-tab${tab.dirty ? " is-dirty" : ""}`}>
      <span className="code-viewer-tab-select"><span className="code-viewer-tab-name">{tab.title}</span>
        {tab.dirty && <span className="code-viewer-tab-dirty">*</span>}
        {tab.readOnly && <span className="code-viewer-tab-readonly">只读</span>}</span>
      <span className="code-viewer-tab-close"><X size={12} strokeWidth={1.8} /></span>
    </div>)}
  </div>;

  if (!tabs.length) return <div ref={rootRef} className="code-viewer-tab-strip is-empty" aria-hidden="true">{measurements}</div>;
  return <div ref={rootRef} className="code-viewer-tab-strip" role="tablist" aria-label={label}>
    {measurements}
    {visible.map((tab, index) => {
      const active = tab.path === activePath;
      // 剩余空间不够整个标签时交给 flex 自己收缩，别在右侧留一条空缝。
      const squeezed = Boolean(flexible && flexible.index === index);
      return <div key={tab.path} className={`code-viewer-tab${active ? " is-active" : ""}${tab.dirty ? " is-dirty" : ""}${squeezed ? " is-squeezed" : ""}`}>
        <Button type="button" variant="bare" role="tab" aria-selected={active} tabIndex={active ? 0 : -1}
          className="code-viewer-tab-select" title={tab.path}
          onClick={() => onSelect(tab.path)}
          onContextMenu={(event) => openMenu(tab.path, event)}
          onAuxClick={(event: MouseEvent<HTMLButtonElement>) => { if (event.button === 1) { event.preventDefault(); onClose(tab.path); } }}
          onKeyDown={(event) => {
            if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
            event.preventDefault();
            const index = tabs.findIndex((item) => item.path === tab.path);
            const next = tabs[(index + (event.key === "ArrowRight" ? 1 : -1) + tabs.length) % tabs.length];
            if (next) onSelect(next.path);
          }}>
          <span className="code-viewer-tab-name">{tab.title}</span>
          {tab.dirty && <span className="code-viewer-tab-dirty" aria-label="有未保存的修改">*</span>}
          {tab.readOnly && <span className="code-viewer-tab-readonly" title="只读">只读</span>}
        </Button>
        <Button type="button" variant="bare" className="code-viewer-tab-close" aria-label={`关闭 ${tab.title}`} title="关闭标签"
          onClick={() => onClose(tab.path)}><X size={12} strokeWidth={1.8} /></Button>
      </div>;
    })}
    {!!hidden.length && <>
      <button ref={moreRef} type="button" className="code-viewer-tab-more" aria-label={`还有 ${hidden.length} 个文件`}
        title={`还有 ${hidden.length} 个文件`} aria-haspopup="dialog" aria-expanded={overflowOpen}
        onClick={() => setOverflowOpen((open) => !open)}><OverflowDots /></button>
      {overflowOpen && <ToolbarPopover anchor={moreRef.current!} label="其他已打开的文件" className="code-viewer-tab-overflow-popover" onClose={() => setOverflowOpen(false)}>
        <div className="code-toolbar-popover-heading"><strong>其他已打开的文件</strong></div>
        <div className="code-viewer-tab-overflow-list" role="list">
          {hidden.map((tab) => <div key={tab.path} className={`code-viewer-tab-overflow-row${tab.path === activePath ? " is-active" : ""}`} role="listitem">
            <Button type="button" variant="bare" className="code-viewer-tab-overflow-select" title={tab.path}
              onClick={() => { (onSelectFromOverflow || onSelect)(tab.path); setOverflowOpen(false); }}
              onContextMenu={(event) => openMenu(tab.path, event)}>
              <span className="code-viewer-tab-name">{tab.title}</span>
              {tab.dirty && <span className="code-viewer-tab-dirty" aria-label="有未保存的修改">*</span>}
              {tab.readOnly && <span className="code-viewer-tab-readonly" title="只读">只读</span>}
            </Button>
            <Button type="button" variant="bare" className="code-viewer-tab-close" aria-label={`关闭 ${tab.title}`} title="关闭标签"
              onClick={() => onClose(tab.path)}><X size={12} strokeWidth={1.8} /></Button>
          </div>)}
        </div>
      </ToolbarPopover>}
    </>}
    {menu && onCloseMany && (() => {
      const order = tabs.map((tab) => tab.path);
      const right = pathsToClose(order, menu.path, "right");
      const others = pathsToClose(order, menu.path, "others");
      const closeMany = (paths: readonly string[], action: TabCloseAction) => {
        if (!paths.length) return;
        setOverflowOpen(false);
        onCloseMany(paths, action);
      };
      return <ContextMenu x={menu.x} y={menu.y} label="标签操作" trigger={menu.trigger} onClose={() => setMenu(null)} items={[
        { label: "关闭右侧标签", disabled: !right.length, onSelect: () => closeMany(right, "right") },
        { label: "关闭其他标签", disabled: others.length < 1, onSelect: () => closeMany(others, "others") },
        { label: "关闭全部标签", disabled: !order.length, onSelect: () => closeMany(order, "all") },
      ]} />;
    })()}
  </div>;
}
