import { useLayoutEffect, useState, type RefObject } from "react";
import { createPortal } from "react-dom";
import { MarkdownDiagram } from "./MarkdownDiagram";

export function useDiagramPortals(root: RefObject<HTMLElement>, streaming = false) {
  const [hosts, setHosts] = useState<HTMLElement[]>([]);
  useLayoutEffect(() => {
    const element = root.current;
    if (!element) return;
    const scan = () => {
      const next = Array.from(element.querySelectorAll<HTMLElement>(".markdown-diagram-mount"));
      setHosts((previous) => previous.length === next.length && previous.every((host, index) => host === next[index]) ? previous : next);
    };
    scan();
    const observer = new MutationObserver(scan);
    observer.observe(element, { childList: true, subtree: true });
    return () => observer.disconnect();
  });
  return hosts.map((host, index) => createPortal(<MarkdownDiagram source={host.dataset.diagramSource || ""} streaming={streaming} />, host, String(index)));
}
