import { createContext, useCallback, useContext, useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { ClipboardList, FolderOpen, ListPlus, LoaderCircle, MessageCircleQuestion, ShieldCheck } from "lucide-react";
import { NoticePanel, type NoticeTone } from "@/components/ui/NoticePanel";
import type { ToolGroupPresentationState } from "./timelinePresentation";

type NoticeKind = "plan" | "background" | "queue" | "permission" | "question" | "compaction" | "files";
type Entry = { id: string; kind: NoticeKind; label: string; summary: string; count?: number; running?: boolean; attention?: boolean; state?: ToolGroupPresentationState };
type RegisteredEntry = Entry & { ownerId: string; preview: boolean };
type DockContext = {
  selected: string;
  host: HTMLDivElement | null;
  owners: Record<string, string>;
  open: (id: string) => void;
  close: () => void;
  register: (entry: RegisteredEntry, autoOpenKey?: string) => void;
  unregister: (ownerId: string) => void;
};
const Context = createContext<DockContext | null>(null);
const PreviewContext = createContext(false);

/** Preview data fills missing kinds; a live request always owns its notice. */
export function ComposerNoticePreview({ children }: { children: ReactNode }) {
  return <PreviewContext.Provider value>{children}</PreviewContext.Provider>;
}
const presentation: Record<NoticeKind, { icon: typeof ShieldCheck; tone: NoticeTone }> = {
  permission: { icon: ShieldCheck, tone: "attention" },
  question: { icon: MessageCircleQuestion, tone: "warning" },
  plan: { icon: ClipboardList, tone: "neutral" },
  background: { icon: LoaderCircle, tone: "success" },
  queue: { icon: ListPlus, tone: "neutral" },
  files: { icon: FolderOpen, tone: "neutral" },
  compaction: { icon: LoaderCircle, tone: "neutral" },
};
const order: NoticeKind[] = ["files", "permission", "question", "plan", "background", "compaction", "queue"];

/** A shared floating position for independent notice cards above the composer. */
export function ComposerNoticeDock({ children }: { children: ReactNode }) {
  const [entries, setEntries] = useState<RegisteredEntry[]>([]);
  const [selected, setSelected] = useState("");
  const [host, setHost] = useState<HTMLDivElement | null>(null);
  const anchor = useRef<HTMLDivElement>(null);
  const seenAutoOpen = useRef(new Map<string, string>());
  const register = useCallback((entry: RegisteredEntry, autoOpenKey?: string) => {
    setEntries((current) => {
      const old = current.find((value) => value.ownerId === entry.ownerId);
      if (old && Object.keys(entry).every((key) => old[key as keyof RegisteredEntry] === entry[key as keyof RegisteredEntry])) return current;
      return [...current.filter((value) => value.ownerId !== entry.ownerId), entry];
    });
    if (!entry.preview && autoOpenKey && seenAutoOpen.current.get(entry.ownerId) !== autoOpenKey) {
      seenAutoOpen.current.set(entry.ownerId, autoOpenKey);
      setSelected(entry.id);
    }
  }, []);
  const unregister = useCallback((ownerId: string) => {
    setEntries((current) => current.filter((entry) => entry.ownerId !== ownerId));
    seenAutoOpen.current.delete(ownerId);
  }, []);
  const owners = useMemo(() => {
    const active = new Map<string, RegisteredEntry>();
    for (const entry of entries) {
      if (!active.has(entry.id) || !entry.preview) active.set(entry.id, entry);
    }
    return Object.fromEntries([...active].map(([id, entry]) => [id, entry.ownerId]));
  }, [entries]);
  useEffect(() => {
    if (selected && !owners[selected]) setSelected("");
  }, [owners, selected]);
  const open = useCallback((id: string) => setSelected(id), []);
  const close = useCallback(() => setSelected(""), []);
  const context = useMemo(() => ({ selected, host, owners, open, close, register, unregister }), [selected, host, owners, open, close, register, unregister]);

  useLayoutEffect(() => {
    const dock = anchor.current;
    const footer = dock?.closest(".chat-footer");
    const workspace = dock?.closest(".chat-workspace") as HTMLElement | null;
    if (!dock || !footer) return;
    let frame: number | null = null;
    const update = () => {
      frame = null;
      const shell = footer.querySelector<HTMLElement>(".composer-shell");
      if (shell) {
        const style = getComputedStyle(shell);
        dock.style.setProperty("--composer-notice-inset-left", style.borderTopLeftRadius);
        dock.style.setProperty("--composer-notice-inset-right", style.borderTopRightRadius);
      }
      const top = Math.max(8, workspace?.getBoundingClientRect().top ?? 8);
      const available = Math.max(0, footer.getBoundingClientRect().top - top - 14);
      const backgroundOpen = Boolean(dock.querySelector('[data-notice-kind="background"] .is-expanded'));
      const maximum = `${Math.floor(Math.min(backgroundOpen ? 560 : 420, window.innerHeight * (backgroundOpen ? .65 : .5), available))}px`;
      const height = `${Math.ceil(dock.getBoundingClientRect().height)}px`;
      if (dock.style.getPropertyValue("--composer-notice-max-height") !== maximum) dock.style.setProperty("--composer-notice-max-height", maximum);
      if (workspace && workspace.style.getPropertyValue("--composer-notice-height") !== height) workspace.style.setProperty("--composer-notice-height", height);
    };
    // ResizeObserver delivery must not synchronously resize its own targets.
    const schedule = () => { if (frame === null) frame = window.requestAnimationFrame(update); };
    const observer = new ResizeObserver(schedule);
    observer.observe(dock);
    observer.observe(footer);
    if (workspace) observer.observe(workspace);
    const terminal = workspace?.querySelector<HTMLElement>(".chat-terminal-panel");
    if (terminal) observer.observe(terminal);
    update();
    window.addEventListener("resize", schedule);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", schedule);
      if (frame !== null) window.cancelAnimationFrame(frame);
      workspace?.style.removeProperty("--composer-notice-height");
    };
  }, []);

  return <Context.Provider value={context}>
    <div ref={anchor} className={`composer-notice-dock${entries.length ? "" : " is-empty"}`} aria-label="对话任务与提醒">
      <div ref={setHost} className="composer-notice-list" />
    </div>
    {children}
  </Context.Provider>;
}

type SlotRenderProps = {
  expanded: boolean;
  setExpanded: (next: boolean) => void;
  contentId: string;
};

export function ComposerNoticeSlot({ children, autoOpenKey, inline = false, compact = false, independent = false, collapsible = false, defaultExpanded = false, onActivate, onCollapse, actions, meta, badge, summaryContent, render, ...entry }: Entry & { children?: ReactNode; autoOpenKey?: string; inline?: boolean; compact?: boolean; independent?: boolean; collapsible?: boolean; defaultExpanded?: boolean; onActivate?: (expanded: boolean) => void; onCollapse?: () => void; actions?: ReactNode | ((open: () => void) => ReactNode); meta?: ReactNode; badge?: ReactNode; summaryContent?: ReactNode; render?: (props: SlotRenderProps) => ReactNode }) {
  const dock = useContext(Context);
  const ownerId = useId();
  const preview = useContext(PreviewContext);
  const row = useRef<HTMLDivElement>(null);
  const [ownExpanded, setOwnExpanded] = useState(false);
  // 独立卡（文件改动）不参与停靠区的「同时只开一个」：它的展开/收起由自己管，
  // 不牵动其它通知的折叠状态。
  const expanded = independent ? ownExpanded : dock?.selected === entry.id;
  const wasExpanded = useRef(expanded);
  if (!dock) throw new Error("ComposerNoticeSlot must be inside ComposerNoticeDock");
  const { register, unregister } = dock;
  useLayoutEffect(() => { register({ ...entry, ownerId, preview }, autoOpenKey); }, [register, ownerId, preview, entry.id, entry.kind, entry.label, entry.summary, entry.count, entry.running, entry.attention, entry.state, autoOpenKey]);
  useLayoutEffect(() => () => unregister(ownerId), [unregister, ownerId]);
  useEffect(() => {
    if (wasExpanded.current && !expanded) onCollapse?.();
    wasExpanded.current = expanded;
  }, [expanded, onCollapse]);
  const { icon: Icon, tone } = presentation[entry.kind];
  const changeExpanded = (next: boolean) => {
    if (independent) {
      setOwnExpanded(next);
      onActivate?.(next);
      return;
    }
    if (next) dock.open(entry.id);
    else dock.close();
    onActivate?.(next);
  };
  useLayoutEffect(() => {
    if (inline || !expanded || !dock.host || !row.current) return;
    const rect = row.current.getBoundingClientRect();
    const hostRect = dock.host.getBoundingClientRect();
    const offset = rect.top - hostRect.top;
    if (offset < 0) dock.host.scrollTop += offset;
    else if (rect.bottom > hostRect.bottom) dock.host.scrollTop += Math.min(offset, rect.bottom - hostRect.bottom);
  }, [inline, expanded, autoOpenKey, dock.host]);
  if (!dock.host || dock.owners[entry.id] !== ownerId) return null;
  if (render) {
    const slotOrder = order.indexOf(entry.kind);
    return createPortal(
      <div
        ref={row}
        className="composer-notice-slot"
        data-notice-kind={entry.kind}
        style={{ order: slotOrder < 0 ? order.length : slotOrder }}
      >
        {render({ expanded, setExpanded: changeExpanded, contentId: `${entry.id}-body` })}
      </div>,
      dock.host,
    );
  }
  const defaultOrder = order.indexOf(entry.kind);
  return createPortal(<div ref={row} className="composer-notice-slot" data-notice-kind={entry.kind} style={{ order: defaultOrder < 0 ? order.length : defaultOrder }}>
    <NoticePanel
      label={entry.label}
      summary={entry.summary}
      summaryContent={summaryContent}
      icon={<Icon size={15} />}
      tone={tone}
      count={entry.count}
      countSuffix={entry.kind === "files" ? "个文件" : undefined}
      compact={compact}
      inline={inline}
      collapsible={collapsible}
      defaultExpanded={defaultExpanded}
      expanded={expanded}
      onExpandedChange={changeExpanded}
      meta={meta}
      badge={badge}
      actions={typeof actions === "function" ? actions(() => dock.open(entry.id)) : actions}
    >
      {children}
    </NoticePanel>
  </div>, dock.host);
}
