import React, { useEffect, useId, useRef, useState } from "react";
import { createPortal } from "react-dom";
import "./tooltip.css";

export function Tooltip({ content, children, className = "" }: {
  content: string;
  children: React.ReactNode;
  className?: string;
}) {
  const id = useId();
  const trigger = useRef<HTMLSpanElement>(null);
  const [position, setPosition] = useState<React.CSSProperties>();
  const show = () => {
    const rect = trigger.current?.getBoundingClientRect();
    if (!rect) return;
    const width = Math.min(360, window.innerWidth - 24);
    setPosition({ width, left: Math.max(12, Math.min(rect.left + rect.width / 2 - width / 2, window.innerWidth - width - 12)),
      ...(rect.top > 130 ? { bottom: window.innerHeight - rect.top + 8 } : { top: rect.bottom + 8 }) });
  };
  const hide = () => setPosition(undefined);
  useEffect(() => {
    if (!position) return;
    const onKeyDown = (event: KeyboardEvent) => { if (event.key === "Escape") hide(); };
    window.addEventListener("resize", hide);
    window.addEventListener("scroll", hide, true);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("resize", hide);
      window.removeEventListener("scroll", hide, true);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [position]);
  return <span ref={trigger} className={`ui-tooltip-trigger ${className}`} tabIndex={0}
    aria-describedby={position ? id : undefined} onMouseEnter={show} onMouseLeave={hide} onFocus={show} onBlur={hide}>
    {children}
    {position && createPortal(<span id={id} role="tooltip" className="ui-tooltip" style={position}>{content}</span>, document.body)}
  </span>;
}
