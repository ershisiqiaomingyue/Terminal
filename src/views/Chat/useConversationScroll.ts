import {
  type KeyboardEvent,
  type TouchEvent,
  type UIEvent,
  type WheelEvent,
  useCallback,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import {
  automaticFollowAfterScroll,
  hasMeaningfulScrollDelta,
  isAwayFromBottom,
  scrollOwnerConsumesDelta,
  shouldScheduleAutomaticFollow,
} from "./conversationScrollState";

type ScrollMessage = { id: string; role: string };

const USER_SCROLL_INTENT_WINDOW_MS = 600;
const SCROLL_KEYS = new Set(["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End", " "]);
const UPWARD_SCROLL_KEYS = new Set(["ArrowUp", "PageUp", "Home"]);

function isInteractiveTarget(target: EventTarget | null) {
  return target instanceof Element
    && target.closest("input, textarea, select, button, [contenteditable='true']") !== null;
}

/**
 * 这次滚轮是不是内层滚动区（执行过程详情、实时输出、长代码块…）自己会消化的？
 *
 * 滚轮事件会冒泡到对话视口，若不区分，在内层面板里往回滚一眼就会把对话的
 * 自动跟随暂停掉，之后再展开思考或工具组就不会跟着往下走了。
 */
function innerScrollOwnerConsumesWheel(event: WheelEvent<HTMLDivElement>, deltaY: number) {
  const viewport = event.currentTarget;
  let node = event.target instanceof Element ? event.target : null;
  while (node && node !== viewport) {
    if (scrollOwnerConsumesDelta(node.scrollHeight, node.clientHeight, node.scrollTop, deltaY)) return true;
    node = node.parentElement;
  }
  return false;
}

export function useConversationScroll({
  sessionId,
  messages,
  layoutVersion,
  terminalOpen,
}: {
  sessionId: string;
  messages: ScrollMessage[];
  layoutVersion: string;
  terminalOpen: boolean;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const followIntentRef = useRef(true);
  const followFrameRef = useRef<number | null>(null);
  const userScrollIntentTimerRef = useRef<number | null>(null);
  const explicitPausePendingRef = useRef(false);
  const userScrollPendingRef = useRef(false);
  const userScrollIntentUntilRef = useRef(0);
  const lastObservedScrollTopRef = useRef(0);
  const lastTouchYRef = useRef<number | null>(null);
  const activeSessionIdRef = useRef(sessionId);
  const [showScrollToBottom, setShowScrollToBottom] = useState(false);
  const [activeUserMessageId, setActiveUserMessageId] = useState("");

  const syncActiveUserMessage = useCallback(() => {
    const viewport = scrollRef.current;
    if (!viewport) return;
    const anchor = viewport.getBoundingClientRect().top + 72;
    const rows = [...viewport.querySelectorAll<HTMLElement>(".message-row.user[data-message-id]")];
    const active = rows.reduce<{ id: string; distance: number } | null>((closest, row) => {
      const id = row.dataset.messageId || "";
      if (!id) return closest;
      const distance = Math.abs(row.getBoundingClientRect().top - anchor);
      return !closest || distance < closest.distance ? { id, distance } : closest;
    }, null);
    if (active) setActiveUserMessageId(active.id);
  }, []);

  const clearPendingUserScroll = useCallback(() => {
    userScrollPendingRef.current = false;
    if (userScrollIntentTimerRef.current != null) {
      window.clearTimeout(userScrollIntentTimerRef.current);
      userScrollIntentTimerRef.current = null;
    }
  }, []);

  const syncScrollToBottomVisibility = useCallback(() => {
    const viewport = scrollRef.current;
    setShowScrollToBottom(Boolean(viewport && !followIntentRef.current
      && isAwayFromBottom(viewport.scrollHeight, viewport.scrollTop, viewport.clientHeight)));
  }, []);

  const scheduleFollowBottom = useCallback(() => {
    syncScrollToBottomVisibility();
    if (!shouldScheduleAutomaticFollow(followIntentRef.current, userScrollPendingRef.current)) return;
    const requestedSessionId = activeSessionIdRef.current;
    const followBottom = () => {
      const viewport = scrollRef.current;
      if (!viewport
        || !shouldScheduleAutomaticFollow(followIntentRef.current, userScrollPendingRef.current)
        || activeSessionIdRef.current !== requestedSessionId) return;
      viewport.scrollTop = Math.max(0, viewport.scrollHeight - viewport.clientHeight);
      lastObservedScrollTopRef.current = viewport.scrollTop;
      setShowScrollToBottom(false);
      syncActiveUserMessage();
    };
    // Layout effects and ResizeObserver run before paint. Scroll immediately:
    // deferring the first correction to rAF exposes a frame of new text below
    // the composer on every fast stream update. Keep one rAF reconciliation
    // for the virtualizer's later row measurements, not the initial follow.
    followBottom();
    if (followFrameRef.current != null) return;
    followFrameRef.current = window.requestAnimationFrame(() => {
      followFrameRef.current = null;
      followBottom();
    });
  }, [syncActiveUserMessage, syncScrollToBottomVisibility]);

  const scrollToLatest = useCallback((force = false) => {
    if (!force && !followIntentRef.current) return;
    clearPendingUserScroll();
    explicitPausePendingRef.current = false;
    lastTouchYRef.current = null;
    userScrollIntentUntilRef.current = 0;
    followIntentRef.current = true;
    setShowScrollToBottom(false);
    scheduleFollowBottom();
  }, [clearPendingUserScroll, scheduleFollowBottom]);

  const markUserScrollIntent = useCallback(() => {
    userScrollPendingRef.current = true;
    userScrollIntentUntilRef.current = performance.now() + USER_SCROLL_INTENT_WINDOW_MS;
    lastObservedScrollTopRef.current = scrollRef.current?.scrollTop ?? lastObservedScrollTopRef.current;
    if (followFrameRef.current != null) {
      window.cancelAnimationFrame(followFrameRef.current);
      followFrameRef.current = null;
    }
    if (userScrollIntentTimerRef.current != null) window.clearTimeout(userScrollIntentTimerRef.current);
    userScrollIntentTimerRef.current = window.setTimeout(() => {
      userScrollIntentTimerRef.current = null;
      userScrollPendingRef.current = false;
      explicitPausePendingRef.current = false;
      scheduleFollowBottom();
    }, USER_SCROLL_INTENT_WINDOW_MS);
  }, [scheduleFollowBottom]);

  const onScrollKeyDown = useCallback((event: KeyboardEvent<HTMLDivElement>) => {
    if (!SCROLL_KEYS.has(event.key) || isInteractiveTarget(event.target)) return;
    markUserScrollIntent();
    const movesUp = UPWARD_SCROLL_KEYS.has(event.key) || (event.key === " " && event.shiftKey);
    explicitPausePendingRef.current = movesUp && event.currentTarget.scrollTop > 0;
    if (explicitPausePendingRef.current) {
      followIntentRef.current = false;
      syncScrollToBottomVisibility();
    }
  }, [markUserScrollIntent, syncScrollToBottomVisibility]);

  const onScrollTouchStart = useCallback((event: TouchEvent<HTMLDivElement>) => {
    lastTouchYRef.current = event.touches[0]?.clientY ?? null;
    explicitPausePendingRef.current = false;
    markUserScrollIntent();
  }, [markUserScrollIntent]);

  const onScrollTouchMove = useCallback((event: TouchEvent<HTMLDivElement>) => {
    const nextTouchY = event.touches[0]?.clientY ?? null;
    const previousTouchY = lastTouchYRef.current;
    lastTouchYRef.current = nextTouchY;
    markUserScrollIntent();
    explicitPausePendingRef.current = nextTouchY != null
      && previousTouchY != null
      && nextTouchY > previousTouchY
      && event.currentTarget.scrollTop > 0;
    if (explicitPausePendingRef.current) {
      followIntentRef.current = false;
      syncScrollToBottomVisibility();
    }
  }, [markUserScrollIntent, syncScrollToBottomVisibility]);

  const onScrollWheel = useCallback((event: WheelEvent<HTMLDivElement>) => {
    if (event.deltaY === 0) return;
    // 内层面板（执行过程详情、实时输出、长代码块）自己会消化这次滚动，
    // 那是在看面板内容，不是在往回翻对话，不能暂停自动跟随。
    if (innerScrollOwnerConsumesWheel(event, event.deltaY)) return;
    markUserScrollIntent();
    explicitPausePendingRef.current = event.deltaY < 0 && event.currentTarget.scrollTop > 0;
    if (explicitPausePendingRef.current) {
      followIntentRef.current = false;
      syncScrollToBottomVisibility();
    }
  }, [markUserScrollIntent, syncScrollToBottomVisibility]);

  const suspendAutomaticFollow = useCallback(() => {
    clearPendingUserScroll();
    explicitPausePendingRef.current = false;
    followIntentRef.current = false;
    syncScrollToBottomVisibility();
    if (followFrameRef.current != null) {
      window.cancelAnimationFrame(followFrameRef.current);
      followFrameRef.current = null;
    }
  }, [clearPendingUserScroll, syncScrollToBottomVisibility]);

  const preserveTimelinePosition = useCallback((_anchor?: HTMLElement) => {
    const viewport = scrollRef.current;
    // 展开/收起是读者在列表里直接点出来的：点击那一刻只要视口还贴着底部，就明确
    // 恢复跟随——否则之前在内层面板里滚过一眼留下的暂停状态，会把刚展开的内容
    // 留在视野之外，看起来就像「展开后没有往下滚」。
    if (viewport && !isAwayFromBottom(viewport.scrollHeight, viewport.scrollTop, viewport.clientHeight)) {
      clearPendingUserScroll();
      explicitPausePendingRef.current = false;
      followIntentRef.current = true;
    }
    scheduleFollowBottom();
  }, [clearPendingUserScroll, scheduleFollowBottom]);

  const onScroll = useCallback((event: UIEvent<HTMLDivElement>) => {
    const viewport = event.currentTarget;
    const previousScrollTop = lastObservedScrollTopRef.current;
    const scrollDelta = viewport.scrollTop - previousScrollTop;
    const userIntent = performance.now() <= userScrollIntentUntilRef.current;
    if (userScrollPendingRef.current && !hasMeaningfulScrollDelta(scrollDelta)) {
      syncScrollToBottomVisibility();
      syncActiveUserMessage();
      return;
    }
    const awayFromBottom = isAwayFromBottom(viewport.scrollHeight, viewport.scrollTop, viewport.clientHeight);
    followIntentRef.current = automaticFollowAfterScroll(
      followIntentRef.current,
      userIntent,
      awayFromBottom,
      scrollDelta,
      explicitPausePendingRef.current,
    );
    explicitPausePendingRef.current = false;
    clearPendingUserScroll();
    if (userIntent) userScrollIntentUntilRef.current = performance.now() + USER_SCROLL_INTENT_WINDOW_MS;
    lastObservedScrollTopRef.current = viewport.scrollTop;
    setShowScrollToBottom(awayFromBottom && !followIntentRef.current);
    syncActiveUserMessage();
  }, [clearPendingUserScroll, syncActiveUserMessage, syncScrollToBottomVisibility]);

  const scrollToMessage = useCallback((messageId: string) => {
    const row = [...(scrollRef.current?.querySelectorAll<HTMLElement>(".message-row[data-message-id]") || [])]
      .find((candidate) => candidate.dataset.messageId === messageId);
    if (!row) return;
    suspendAutomaticFollow();
    row.scrollIntoView({ block: "center" });
  }, [suspendAutomaticFollow]);

  useLayoutEffect(() => {
    activeSessionIdRef.current = sessionId;
    followIntentRef.current = true;
    clearPendingUserScroll();
    explicitPausePendingRef.current = false;
    lastTouchYRef.current = null;
    userScrollIntentUntilRef.current = 0;
    lastObservedScrollTopRef.current = scrollRef.current?.scrollTop ?? 0;
    setActiveUserMessageId("");
    setShowScrollToBottom(false);
    if (followFrameRef.current != null) {
      window.cancelAnimationFrame(followFrameRef.current);
      followFrameRef.current = null;
    }
    scheduleFollowBottom();
  }, [clearPendingUserScroll, scheduleFollowBottom, sessionId]);

  useLayoutEffect(() => {
    scheduleFollowBottom();
  }, [layoutVersion, messages.length, scheduleFollowBottom]);

  useLayoutEffect(() => {
    scheduleFollowBottom();
  }, [scheduleFollowBottom, terminalOpen]);

  const conversationIsEmpty = messages.length === 0;
  useLayoutEffect(() => {
    const viewport = scrollRef.current;
    const content = viewport?.querySelector(".message-list");
    if (!viewport || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => scheduleFollowBottom());
    observer.observe(viewport);
    if (content) observer.observe(content);
    return () => observer.disconnect();
  }, [conversationIsEmpty, scheduleFollowBottom, sessionId]);

  useLayoutEffect(() => () => {
    if (followFrameRef.current != null) window.cancelAnimationFrame(followFrameRef.current);
    if (userScrollIntentTimerRef.current != null) window.clearTimeout(userScrollIntentTimerRef.current);
  }, []);

  return {
    activeUserMessageId,
    markUserScrollIntent,
    onScroll,
    onScrollKeyDown,
    onScrollTouchMove,
    onScrollTouchStart,
    onScrollWheel,
    preserveTimelinePosition,
    scheduleFollowBottom,
    scrollRef,
    scrollToLatest,
    scrollToMessage,
    setShowScrollToBottom,
    showScrollToBottom,
    suspendAutomaticFollow,
  };
}
