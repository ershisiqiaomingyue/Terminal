import { useEffect, useMemo, useRef, useState } from "react";
import { Button } from "rain-ui";
import { chooseDirectory } from "@/platform/api";
import { readHostStatus } from "@/platform/hostStatus";
import { ContextMenu, type ContextMenuItem } from "rain-ui";
import { RenameDialog } from "rain-ui";
import { ArchivedSessionsDialog } from "@/components/ArchivedSessionsDialog/ArchivedSessionsDialog";
import {
  ArchiveIcon,
  BranchIcon,
  CloseIcon,
  CopyIcon,
  EditIcon,
  EllipsisIcon,
  FolderCloseIcon,
  FolderOpenIcon,
  NewChatIcon,
  PinFillIcon,
  ProjectAddIcon,
  SearchIcon,
  TrashIcon,
  TriangleRightFillIcon,
} from "@/components/Sidebar/navIcons";
import type { ChatSession } from "./types";
import { formatSessionAge, orderNavSessions } from "./sessionNavPresentation";
import { useConversationRunStatuses } from "./useConversationRunStatuses";
import { sessionRowStatus, visibleSessionLimit, SESSION_WATERMARK_BASE, type RowState, type RunStatusView } from "./sessionRowStatus";
import { useTitleMarquee } from "./useTitleMarquee";
import { workspaceKey } from "./chatIdentity";
import {
  CHAT_NAV_EVENT,
  dispatchChatNav,
  readChatNavSnapshot,
  readChatSessions,
  readWorkspaces,
  type ChatNavDetail,
} from "./chatNavStore";
import "./workspace-nav.css";

const prefetchConversation = (id: string) => import("./preloadConversation")
  .then(module => module.prefetchConversation(id)).catch(() => null);

/** 常驻分组：没有它就补一条，界面上永远能看到这个入口。 */
const DEFAULT_WORKSPACE_NAME = "默认工作区";

function cx(...values: Array<string | false | null | undefined>): string {
  return values.filter(Boolean).join(" ");
}

interface ConversationNavProps {
  onOpenChat: () => void;
}

/** 侧栏菜单分两类：会话行的 ⋯ 和工作区行的 ⋯ 打开的是不同的菜单。 */
type NavMenu =
  | { kind: "session"; id: string; x: number; y: number; trigger: HTMLElement }
  | { kind: "project"; path: string; x: number; y: number; trigger: HTMLElement };

/* -------------------------------------------------------------------------- */
/* 状态圆点 / 转圈：照抄 dsh 的 StateDot                                       */
/* -------------------------------------------------------------------------- */

/**
 * 行首状态标记。
 *
 * - `warning`（等待回答 / 等待审批）：10px 槽位里 6px 实心 amber 圆点，带一个很轻的
 *   呼吸节奏，让「有东西等你」在静止的列表里也能被余光注意到；
 * - `ongoing`（进行中）：14px 圆环，缺口靠 stroke-dasharray 呼吸——dsh 用同一个
 *   环表示运行中，这里连动画节奏一并照搬。
 */
function StateDot({ state, label }: { state: RowState; label: string }) {
  if (state === "ongoing") {
    return <span className="ws-nav-spinner" role="img" aria-label={label} title={label}>
      <svg viewBox="0 0 24 24" fill="none" aria-hidden="true">
        <g className="ws-nav-spinner-motion">
          <circle className="ws-nav-spinner-track" cx="12" cy="12" r="9.5" stroke="currentColor" strokeWidth="2" />
          <circle className="ws-nav-spinner-arc" cx="12" cy="12" r="9.5" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
        </g>
      </svg>
    </span>;
  }
  return <span className="ws-nav-dot" data-state={state} role="img" aria-label={label} title={label} />;
}

/* -------------------------------------------------------------------------- */
/* 会话行                                                                      */
/* -------------------------------------------------------------------------- */

interface SessionRowProps {
  session: ChatSession;
  status: RunStatusView | undefined;
  selected: boolean;
  menuOpen: boolean;
  now: number;
  onOpen: () => void;
  onPrefetch: () => void;
  onBeginRename: () => void;
  onOpenMenu: (trigger: HTMLElement) => void;
  onArchive: () => void;
  onTogglePin: () => void;
}

function SessionRow({
  session,
  status,
  selected,
  menuOpen,
  now,
  onOpen,
  onPrefetch,
  onBeginRename,
  onOpenMenu,
  onArchive,
  onTogglePin,
}: SessionRowProps) {
  const titleRef = useRef<HTMLSpanElement>(null);
  const marquee = useTitleMarquee(titleRef);
  const view = sessionRowStatus(status);
  const title = session.title.trim();
  // 行尾常态是「更新到现在」，等待交互时换成紧凑状态词（待回答 / 待审批），
  // 否则「有东西等你」这件事会被一个「3分钟」盖过去。
  const trailing = view.trailingLabel ?? formatSessionAge(session.updatedAt ?? session.createdAt ?? now, now);
  return <div
    data-session-id={session.id}
    role="treeitem"
    aria-selected={selected}
    aria-current={selected ? "page" : undefined}
    tabIndex={0}
    className={cx("ws-nav-session-row", selected && "ws-nav-selected", menuOpen && "ws-nav-menu-open", session.archived && "ws-nav-archived")}
    onPointerEnter={() => { marquee.enter(); onPrefetch(); }}
    onPointerLeave={marquee.leave}
    onFocus={onPrefetch}
    onClick={onOpen}
    onDoubleClick={onBeginRename}
    onKeyDown={(event) => {
      // 行内按钮自己也吃 Enter/Space；不挡住冒泡的话，用键盘开菜单会顺带切走对话。
      if (event.target !== event.currentTarget) return;
      if (event.key === "Enter") { event.preventDefault(); onOpen(); return; }
      if (event.key === "F2") { event.preventDefault(); onBeginRename(); }
    }}
  >
    {/* 槽位固定 16×20：等待交互是 amber 圆点、运行中是转圈、空闲留白。宽度固定，
        所以状态切换不会推动标题。 */}
    <span className="ws-nav-slot">
      {view.state === "idle" ? null : <StateDot state={view.state} label={view.label} />}
    </span>
    <span ref={titleRef} className="ws-nav-title" onDoubleClick={(event) => { event.stopPropagation(); onBeginRename(); }}>
      {title || "未命名"}
    </span>
    <span className="ws-nav-time">{trailing}</span>
    {session.pinned && <span className="ws-nav-pin" role="img" aria-label="已置顶" title="已置顶"><PinFillIcon size={14} /></span>}
    <span className="ws-nav-row-actions" onClick={(event) => event.stopPropagation()} onDoubleClick={(event) => event.stopPropagation()}>
      <Button
        type="button" variant="bare"
        className="ws-nav-row-icon-button"
        title="更多操作"
        aria-label="更多操作"
        aria-haspopup="menu"
        aria-expanded={menuOpen}
        onClick={(event) => onOpenMenu(event.currentTarget)}
      >
        <EllipsisIcon size={16} />
      </Button>
      {/* 归档与置顶是仅次于打开的第二、第三动作，悬停直接给出来，省一次开菜单。
          归档会改变这条对话的去处，悬停用仓库统一的淡红 danger 底色提示后果。 */}
      <Button
        type="button" variant="bare"
        className="ws-nav-row-icon-button ws-nav-row-icon-button-danger"
        title="归档会话"
        aria-label="归档会话"
        onClick={onArchive}
      >
        <ArchiveIcon size={16} />
      </Button>
      <Button
        type="button" variant="bare"
        className="ws-nav-row-icon-button"
        title={session.pinned ? "取消置顶" : "置顶会话"}
        aria-label={session.pinned ? "取消置顶" : "置顶会话"}
        aria-pressed={Boolean(session.pinned)}
        onClick={onTogglePin}
      >
        <PinFillIcon size={16} />
      </Button>
    </span>
  </div>;
}

/* -------------------------------------------------------------------------- */
/* 工作区行                                                                    */
/* -------------------------------------------------------------------------- */

interface ProjectRowProps {
  label: string;
  /** 悬停提示用；未分组这类虚拟分组没有真实路径。 */
  title?: string;
  expanded: boolean;
  active: boolean;
  menuOpen: boolean;
  onToggle: () => void;
  onNewSession: () => void;
  /** 未分组不是注册工作区，没有「移除」这类动作，所以干脆不给菜单按钮。 */
  onOpenMenu?: (trigger: HTMLElement) => void;
}

function ProjectRow({ label, title, expanded, active, menuOpen, onToggle, onNewSession, onOpenMenu }: ProjectRowProps) {
  return <div
    role="treeitem"
    aria-expanded={expanded}
    tabIndex={0}
    title={title}
    className={cx("ws-nav-project-row", menuOpen && "ws-nav-menu-open")}
    onClick={onToggle}
    onKeyDown={(event) => {
      // 同上：行内按钮的 Enter/Space 属于按钮自己，别再折回展开/收起。
      if (event.target !== event.currentTarget) return;
      if (event.key === "Enter" || event.key === " ") { event.preventDefault(); onToggle(); }
    }}
  >
    {/* folder 槽位常态显示，hover 时换成朝右的实心三角（展开转 90°）；两个槽位
        同宽同位，交换不会让整行横向跳动。当前会话所在的工作区用主题色文件夹。 */}
    <span className={cx("ws-nav-slot", "ws-nav-folder", active && "ws-nav-folder-active")}>
      {expanded ? <FolderOpenIcon size={16} /> : <FolderCloseIcon size={16} />}
    </span>
    <span className={cx("ws-nav-slot", "ws-nav-chevron")}>
      <TriangleRightFillIcon className={cx("ws-nav-arrow", expanded && "ws-nav-arrow-open")} />
    </span>
    <span className="ws-nav-project-text"><span className="ws-nav-title">{label}</span></span>
    <span className="ws-nav-row-actions" onClick={(event) => event.stopPropagation()}>
      {onOpenMenu && <Button type="button" variant="bare" className="ws-nav-row-icon-button" title="工作区操作" aria-label="工作区操作" aria-haspopup="menu" aria-expanded={menuOpen} onClick={(event) => onOpenMenu(event.currentTarget)}>
        <EllipsisIcon size={16} />
      </Button>}
      <Button type="button" variant="bare" className="ws-nav-row-icon-button" title="在此工作区新建对话" aria-label="在此工作区新建对话" onClick={onNewSession}>
        <NewChatIcon size={16} />
      </Button>
    </span>
  </div>;
}

/* -------------------------------------------------------------------------- */
/* 面板                                                                        */
/* -------------------------------------------------------------------------- */

export function ConversationNav({ onOpenChat }: ConversationNavProps) {
  const runStatuses = useConversationRunStatuses();
  const [workspaces, setWorkspaces] = useState(readWorkspaces);
  const [sessions, setSessions] = useState(readChatSessions);
  const [activeNav, setActiveNav] = useState(readChatNavSnapshot);
  const [defaultPath, setDefaultPath] = useState("");
  // 记「收起了哪些」而不是「展开了哪些」：工作区默认全部展开，新加的工作区
  // 也自动是展开的，不用逐个补进展开列表。
  const [collapsedProjects, setCollapsedProjects] = useState<string[]>([]);
  /** 超出水位线后点了「展开其余」的分组（按路径键）→ 当前水位线。 */
  const [revealedProjects, setRevealedProjects] = useState<Record<string, number>>({});
  const [searchOpen, setSearchOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [now, setNow] = useState(() => Date.now());
  const [menu, setMenu] = useState<NavMenu | null>(null);
  /** 正在重命名的对话：非空时弹重命名弹窗（行内改名的即视感太挤，见 RenameDialog 注释）。 */
  const [renameTarget, setRenameTarget] = useState<ChatSession | null>(null);
  const [archivedOpen, setArchivedOpen] = useState(false);
  const searchRef = useRef<HTMLInputElement>(null);
  const searchRootRef = useRef<HTMLDivElement>(null);
  /** 供 document 级监听读取最新过滤词，避免把它挂进 effect 依赖里反复重绑。 */
  const normalizedQueryRef = useRef("");

  useEffect(() => {
    const refresh = () => {
      setWorkspaces(readWorkspaces());
      setSessions(readChatSessions());
      setActiveNav(readChatNavSnapshot());
    };
    window.addEventListener(CHAT_NAV_EVENT, refresh);
    return () => window.removeEventListener(CHAT_NAV_EVENT, refresh);
  }, []);

  // 会话行右侧是相对时间，每分钟重算一次就够，不需要跟着每次事件刷新跳动。
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 60_000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => { if (searchOpen) searchRef.current?.focus(); }, [searchOpen]);

  // 点空白处收起搜索：dsh 只在输入框为空时收（有过滤词时贸然收起会把用户的筛选
  // 悄悄丢掉），这里照做，同时把焦点从输入框上摘下来。
  useEffect(() => {
    if (!searchOpen) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!(event.target instanceof Node)) return;
      if (searchRootRef.current?.contains(event.target)) return;
      if (normalizedQueryRef.current) return;
      searchRef.current?.blur();
      setSearchOpen(false);
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, [searchOpen]);

  // 默认工作区照 dsh 的思路：它是宿主给的一个「总在」的目录，不写进工作区列表，
  // 由侧栏自己补出来。优先用户主目录（和真实项目目录区分开），取不到时退回进程
  // 工作目录；浏览器预览拿不到主机信息，就保持空路径——分组本身仍然常驻。
  useEffect(() => {
    let active = true;
    void readHostStatus(1).then((snapshot) => {
      if (!active || !snapshot) return;
      setDefaultPath(snapshot.homeDir || snapshot.workingDir || "");
    }).catch(() => undefined);
    return () => { active = false; };
  }, []);

  const defaultKey = workspaceKey(defaultPath);

  /** 渲染用的工作区列表：注册过的工作区 + 一定存在的默认工作区。 */
  const entries = useMemo(() => {
    const list = workspaces.map((workspace) => ({ path: workspace.path, name: workspace.name, isDefault: false }));
    if (!list.some((entry) => workspaceKey(entry.path) === defaultKey)) {
      list.push({ path: defaultPath, name: DEFAULT_WORKSPACE_NAME, isDefault: true });
    }
    return list;
  }, [workspaces, defaultPath, defaultKey]);

  const grouped = useMemo(() => {
    const projects = new Map<string, ChatSession[]>();
    const registered = new Set(entries.map((entry) => workspaceKey(entry.path)));
    // 没有归属的对话一律收进默认工作区：独立对话本来就没有目录，指向未注册目录的
    // 历史对话同理。以前它们单独成「未分组」，结果顶栏那个「+」建出来的对话跑到
    // 另一组去了，看起来像「加号没加到默认工作区」。
    for (const session of sessions) {
      if (session.archived) continue;
      const key = session.scope === "workspace" ? workspaceKey(session.workspacePath || "") : "";
      const target = registered.has(key) ? key : defaultKey;
      const items = projects.get(target) || [];
      items.push(session);
      projects.set(target, items);
    }
    return new Map([...projects].map(([key, items]) => [key, orderNavSessions(items)]));
  }, [sessions, entries, defaultKey]);

  const send = (detail: ChatNavDetail) => { if (detail.type === "open-session" && detail.id) void prefetchConversation(detail.id); onOpenChat(); dispatchChatNav(detail); };
  // 行内操作（置顶/重命名/归档/复制 ID/移除工作区）只改数据，不把用户从当前页面
  // 拽进对话页。
  const mutate = (detail: ChatNavDetail) => dispatchChatNav(detail);
  const isCollapsed = (path: string) => collapsedProjects.some((item) => workspaceKey(item) === workspaceKey(path));
  const expand = (path: string) => setCollapsedProjects((previous) => previous.filter((item) => workspaceKey(item) !== workspaceKey(path)));
  const toggleProject = (path: string) => setCollapsedProjects((previous) => previous.some((item) => workspaceKey(item) === workspaceKey(path)) ? previous.filter((item) => workspaceKey(item) !== workspaceKey(path)) : [...previous, path]);
  const addWorkspace = async () => {
    const path = await chooseDirectory("选择工作区目录");
    if (!path) return;
    const name = path.split(/[\\/]/).filter(Boolean).pop() || path;
    send({ type: "add-workspace", path, name });
  };
  const commitRename = (session: ChatSession, value: string) => {
    setRenameTarget(null);
    const title = value.trim();
    if (title && title !== session.title) mutate({ type: "rename-session", id: session.id, title });
  };
  const copySessionId = async (id: string) => {
    try { await navigator.clipboard.writeText(id); } catch { /* 没有剪贴板权限时保持静默 */ }
  };
  // 菜单挂在按钮的右下方（左边缘对齐按钮左边缘，往下错开 4px）；超出窗口时
  // ContextMenu 自己会夹回来。
  const openSessionMenu = (session: ChatSession) => (trigger: HTMLElement) => {
    const rect = trigger.getBoundingClientRect();
    setMenu({ kind: "session", id: session.id, x: rect.left, y: rect.bottom + 4, trigger });
  };
  const openProjectMenu = (path: string) => (trigger: HTMLElement) => {
    const rect = trigger.getBoundingClientRect();
    setMenu({ kind: "project", path, x: rect.left, y: rect.bottom + 4, trigger });
  };
  const newSession = (path: string, name: string) => {
    // 浏览器预览里拿不到宿主目录，默认工作区的 path 是空的；这时退回「独立对话」，
    // 否则点了会静默无事发生（对话页那边要求 path 非空才建会话）。
    if (!path) { send({ type: "new-standalone" }); return; }
    expand(path);
    send({ type: "new-workspace-session", path, name });
  };

  const normalized = query.trim().toLocaleLowerCase();
  normalizedQueryRef.current = normalized;
  const matches = (text: string) => text.toLocaleLowerCase().includes(normalized);
  const searching = Boolean(normalized);
  const visibleEntries = entries.filter((entry) => matches(entry.name) || grouped.get(workspaceKey(entry.path))?.some((session: ChatSession) => matches(session.title)));

  const menuTarget = menu?.kind === "session" ? sessions.find((session) => session.id === menu.id) : undefined;
  const menuProject = menu?.kind === "project" ? entries.find((entry) => workspaceKey(entry.path) === workspaceKey(menu.path)) : undefined;
  const menuItems: ContextMenuItem[] = menuTarget ? [
    { label: menuTarget.pinned ? "取消置顶" : "置顶会话", icon: <PinFillIcon size={14} />, onSelect: () => mutate({ type: "pin-session", id: menuTarget.id, next: !menuTarget.pinned }) },
    { label: "重命名", icon: <EditIcon size={14} />, onSelect: () => setRenameTarget(menuTarget) },
    { label: "分叉会话", icon: <BranchIcon size={14} />, onSelect: () => mutate({ type: "fork-session", id: menuTarget.id }) },
    { label: "归档会话", icon: <ArchiveIcon size={14} />, onSelect: () => mutate({ type: "archive-session", id: menuTarget.id }) },
    { label: "复制 Session ID", icon: <CopyIcon size={14} />, onSelect: () => void copySessionId(menuTarget.id) },
    // 彻底删除：走的是对话页那条既有通路（二次确认 + 删历史 + 清缓存），
    // 归档只是从列表里藏起来，真要清干净得从这里点。
    { label: "删除对话", icon: <TrashIcon size={14} />, danger: true, onSelect: () => mutate({ type: "remove-session", id: menuTarget.id }) },
  ] : menuProject ? [
    { label: "新建对话", icon: <NewChatIcon size={14} />, onSelect: () => newSession(menuProject.path, menuProject.name) },
    // 默认工作区要一直在，所以它没有「移除」这一项。
    ...(menuProject.isDefault ? [] : [{ label: "移除工作区", icon: <TrashIcon size={14} />, danger: true, onSelect: () => send({ type: "remove-workspace", path: menuProject.path }) }]),
  ] : [];

  return <div className="ws-nav">
    {/* 顶栏就是「工作区」段的段头（dsh 里段名就长在顶栏上）：左边段名，右边一串
        动作——归档对话、会展开的搜索框、新建独立对话、添加工作区。搜索一展开，
        段名与两侧动作一起收起，只留输入框。 */}
    <div className="ws-nav-header">
      <span className={cx("ws-nav-section-label", searchOpen && "ws-nav-section-label-hidden")}>工作区</span>
      {/* 归档的对话不在侧栏里，得有个常驻入口：紧挨着搜索左侧，和它同属右侧动作区。
          它自己的 margin-left:auto 负责把整串动作推到右边（原来这个 auto 在搜索槽上）。 */}
      <Button
        type="button" variant="bare"
        className={cx("ws-nav-icon-button", "ws-nav-archive-button", searchOpen && "ws-nav-archive-button-hidden")}
        title="归档对话"
        aria-label="归档对话"
        tabIndex={searchOpen ? -1 : 0}
        onClick={() => setArchivedOpen(true)}
      >
        <ArchiveIcon size={16} />
      </Button>
      <div className={cx("ws-nav-search-slot", searchOpen && "ws-nav-search-slot-expanded")}>
        <div ref={searchRootRef} className={cx("ws-nav-search", searchOpen && "ws-nav-search-expanded")} onClick={() => { if (!searchOpen) setSearchOpen(true); }}>
          <Button
            type="button" variant="bare"
            className="ws-nav-search-button"
            title="搜索对话"
            aria-label="搜索对话"
            aria-expanded={searchOpen}
            onClick={(event) => { event.stopPropagation(); setSearchOpen(true); searchRef.current?.focus(); }}
          >
            <SearchIcon size={searchOpen ? 11 : 14} />
          </Button>
          <input
            ref={searchRef}
            className="ws-nav-search-input"
            value={query}
            placeholder="搜索工作区与对话"
            aria-label="搜索工作区与对话"
            tabIndex={searchOpen ? 0 : -1}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => { if (event.key === "Escape") { setQuery(""); setSearchOpen(false); } }}
          />
          {searchOpen && <Button type="button" variant="bare" className="ws-nav-clear-button" title="关闭搜索" aria-label="关闭搜索" onClick={(event) => { event.stopPropagation(); setQuery(""); setSearchOpen(false); }}>
            <CloseIcon size={14} />
          </Button>}
        </div>
      </div>
      <div className={cx("ws-nav-header-actions", searchOpen && "ws-nav-header-actions-hidden")}>
        <Button type="button" variant="bare" className="ws-nav-icon-button" title="新建独立对话" aria-label="新建独立对话" onClick={() => send({ type: "new-standalone" })}>
          <NewChatIcon size={16} />
        </Button>
        <Button type="button" variant="bare" className="ws-nav-icon-button" title="添加工作区" aria-label="添加工作区" onClick={() => void addWorkspace()}>
          <ProjectAddIcon size={16} />
        </Button>
      </div>
    </div>
    <div className="ws-nav-body">
      <nav className="ws-nav-list" role="tree" aria-label="工作区与对话">
        {visibleEntries.map((entry) => {
          const key = workspaceKey(entry.path);
          const items = grouped.get(key) || [];
          const filtered = matches(entry.name) ? items : items.filter((session: ChatSession) => matches(session.title));
          const open = !isCollapsed(entry.path) || searching;
          const active = items.some((session: ChatSession) => session.id === activeNav.sessionId);
          // 水位线：一组默认只露 5 行，但置顶 / 当前 / 等待交互的行会自动抬到
          // 可见区，多出来的收进「展开其余」——否则「有东西等你」那行可能正好被折叠。
          const baseline = visibleSessionLimit(filtered, activeNav.sessionId, (session) => Boolean(runStatuses[session.id]));
          // 搜索时直接铺开全部命中行，让用户一眼看到「到底有几个」，再让他手动收起。
          const limit = searching ? filtered.length : Math.max(baseline, revealedProjects[key] ?? 0);
          const visible = filtered.slice(0, Math.min(filtered.length, limit));
          const hidden = filtered.length - visible.length;
          // 按钮的可见性看「基准水位线有没有藏东西」，而不是「当前水位线」：
          // 否则展开一次之后按钮就消失，再也收不回去（dsh 的收起就是同一颗按钮）。
          const overflow = filtered.length - baseline;
          return <div className="ws-nav-group" role="none" key={entry.path || "__default__"}>
            <ProjectRow
              label={entry.name}
              title={entry.path || undefined}
              expanded={open}
              active={active}
              menuOpen={menu?.kind === "project" && workspaceKey(menu.path) === key}
              onToggle={() => toggleProject(entry.path)}
              onNewSession={() => newSession(entry.path, entry.name)}
              onOpenMenu={openProjectMenu(entry.path)}
            />
            {open && visible.length > 0 && <div className="ws-nav-group-sessions" role="group">
              {visible.map((session) => <SessionRow
                key={session.id}
                session={session}
                status={runStatuses[session.id]}
                selected={session.id === activeNav.sessionId}
                menuOpen={menu?.kind === "session" && menu.id === session.id}
                now={now}
                onOpen={() => send({ type: "open-session", id: session.id })}
                onPrefetch={() => void prefetchConversation(session.id)}
                onBeginRename={() => setRenameTarget(session)}
                onOpenMenu={openSessionMenu(session)}
                onArchive={() => mutate({ type: "archive-session", id: session.id })}
                onTogglePin={() => mutate({ type: "pin-session", id: session.id, next: !session.pinned })}
              />)}
              {overflow > 0 && <Button
                type="button" variant="bare"
                className="ws-nav-overflow"
                aria-expanded={hidden === 0}
                onClick={() => setRevealedProjects((previous) => {
                  // 已经全展开 ⇒ 收回基准水位线；否则按 5 行一档往下放，最后一档直接放完。
                  if (hidden === 0) {
                    const next = { ...previous };
                    delete next[key];
                    return next;
                  }
                  return { ...previous, [key]: hidden <= SESSION_WATERMARK_BASE ? Number.POSITIVE_INFINITY : visible.length + SESSION_WATERMARK_BASE };
                })}
              >
                {hidden === 0 ? "收起" : `展开其余 ${hidden} 个会话`}
              </Button>}
            </div>}
          </div>;
        })}
        {searching && !visibleEntries.length && <p className="ws-nav-empty">没有找到匹配的对话</p>}
      </nav>
      <span className="ws-nav-fade" aria-hidden="true" />
    </div>
    {menu && (menuTarget || menuProject) && <ContextMenu
      x={menu.x}
      y={menu.y}
      label={menuTarget ? `对话「${menuTarget.title}」的操作` : `工作区「${menuProject?.name ?? ""}」的操作`}
      trigger={menu.trigger}
      onClose={() => setMenu(null)}
      items={menuItems}
    />}
    {/* 重命名走弹窗而不是把行改成输入框：行一变输入框，行尾的时间、置顶、三个按钮
        就全得让位，整行会跳一下，光标也常常落不进去。 */}
    <RenameDialog
      open={Boolean(renameTarget)}
      title="重命名对话"
      label="对话名称"
      initialValue={renameTarget?.title ?? ""}
      confirmLabel="重命名"
      onCancel={() => setRenameTarget(null)}
      onConfirm={(value) => { if (renameTarget) commitRename(renameTarget, value); else setRenameTarget(null); }}
    />
    {archivedOpen && <ArchivedSessionsDialog onClose={() => setArchivedOpen(false)} />}
  </div>;
}
