import { useLayoutEffect, type RefObject } from "react";
import { flushSync } from "react-dom";
import { useVirtualizer } from "@tanstack/react-virtual";

export function useConversationVirtualizer({ messages, layoutVersion, scrollRef, followBottom }: {
  messages: { id: string; role: string }[];
  layoutVersion: string;
  scrollRef: RefObject<HTMLDivElement>;
  followBottom: () => void;
}) {
  const virtualizer = useVirtualizer({
    count: messages.length,
    getScrollElement: () => scrollRef.current,
    getItemKey: (index) => messages[index]?.id || index,
    estimateSize: (index) => messages[index]?.role === "user" ? 92 : 220,
    gap: 22,
    overscan: 10,
  });
  const totalHeight = virtualizer.getTotalSize();

  useLayoutEffect(() => {
    const index = messages.length - 1;
    const tail = scrollRef.current?.querySelector<HTMLElement>(`.message-virtual-row[data-index="${index}"]`);
    // The default measurement reuses cached heights until ResizeObserver runs.
    // Streaming must correct the mounted tail and follow before the next paint.
    if (tail) virtualizer.resizeItem(index, tail.offsetHeight);
    followBottom();
  }, [followBottom, layoutVersion, messages.length, scrollRef, totalHeight, virtualizer]);

  const empty = messages.length === 0;
  useLayoutEffect(() => {
    const content = scrollRef.current?.querySelector(".message-list");
    if (!content) return;
    let tail: HTMLElement | null = null;
    const reconcile = () => {
      const index = virtualizer.options.count - 1;
      const nextTail = content.querySelector<HTMLElement>(`.message-virtual-row[data-index="${index}"]`);
      if (nextTail !== tail) {
        if (tail) resize?.unobserve(tail);
        tail = nextTail;
        if (tail) resize?.observe(tail);
      }
      // Worker Markdown, images and diagrams can grow independently of the
      // parent. Commit the corrected list height in this observer callback;
      // an async virtualizer notification would expose a clipped frame.
      if (tail) {
        const height = tail.offsetHeight;
        const cachedHeight = virtualizer.itemSizeCache.get(virtualizer.options.getItemKey(index));
        if (height !== cachedHeight) flushSync(() => virtualizer.resizeItem(index, height));
      }
      followBottom();
    };
    const resize = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(reconcile);
    const mutations = new MutationObserver(reconcile);
    mutations.observe(content, { childList: true, characterData: true, subtree: true });
    // Initial size is already reconciled by the layout effect above. Observers
    // run outside React's commit, where flushSync is safe.
    tail = content.querySelector<HTMLElement>(`.message-virtual-row[data-index="${virtualizer.options.count - 1}"]`);
    if (tail) resize?.observe(tail);
    return () => {
      resize?.disconnect();
      mutations.disconnect();
    };
  }, [empty, followBottom, scrollRef, virtualizer]);

  return virtualizer;
}
