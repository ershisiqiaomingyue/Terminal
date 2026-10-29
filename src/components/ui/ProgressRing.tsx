import React, { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { Button } from "./Button";

/**
 * ProgressRing
 * ---------------------------------------------------------------------------
 * The context-usage control from the composer: a small ring that fills to a
 * percentage, a percentage label beside it, and an expandable breakdown panel.
 *
 * Extracted from ContextUsageIndicator so the ring geometry and the tone
 * thresholds (70% warning, 90% danger) are shared instead of copied per view.
 * Token formatting stays with the caller through `formatValue`.
 */

export type ProgressRingSegment = {
  id: string;
  label: string;
  /** 0-100. Segments are drawn left to right in the order given. */
  percent: number | null;
  /** Optional formatted count; replaces the percentage in the legend. */
  valueText?: React.ReactNode;
};

export type ProgressRingProps = {
  /** 0-100, or null when the value is unknown. */
  percent: number | null;
  /** Accessible name for the trigger. */
  label?: string;
  /** Right-aligned detail in the panel header. */
  valueText?: React.ReactNode;
  segments?: ProgressRingSegment[];
  /** Called when the panel opens, e.g. to refresh a stale reading. */
  onOpen?: () => void;
  /** Adds a pulsing state while a reading is in flight. */
  busy?: boolean;
  panelLabel?: string;
  children?: React.ReactNode;
};

const ringTone = (percent: number | null) =>
  percent == null ? "is-unknown" : percent >= 90 ? "is-danger" : percent >= 70 ? "is-warning" : "";

export const ProgressRing: React.FC<ProgressRingProps> = ({
  percent,
  label,
  valueText,
  segments,
  onOpen,
  busy = false,
  panelLabel = "用量明细",
  children,
}) => {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const panel = useRef<HTMLElement>(null);
  const [placement, setPlacement] = useState<{ left: number; top: number; maxHeight: number }>();
  const panelId = useId();
  const display = percent == null ? "—" : `${percent}%`;
  const accessible = label ?? (percent == null ? "占用未知" : `已用 ${percent}%`);
  const displaySegments = segments ?? [];

  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      if (!root.current || !panel.current) return;
      const anchor = root.current.getBoundingClientRect();
      const maxHeight = Math.max(0, window.innerHeight - 32);
      const height = Math.min(panel.current.offsetHeight, maxHeight);
      const left = Math.max(16, Math.min(anchor.left, window.innerWidth - panel.current.offsetWidth - 16)) - anchor.left;
      const top = Math.max(16, Math.min(anchor.top - height - 8, window.innerHeight - height - 16)) - anchor.top;
      setPlacement((current) => current?.left === left && current.top === top && current.maxHeight === maxHeight
        ? current : { left, top, maxHeight });
    };
    place();
    const observer = new ResizeObserver(place);
    if (panel.current) observer.observe(panel.current);
    if (root.current) observer.observe(root.current);
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) setOpen(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      setOpen(false);
      root.current?.querySelector<HTMLButtonElement>("button")?.focus({ preventScroll: true });
    };
    document.addEventListener("pointerdown", outside, true);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("pointerdown", outside, true);
      document.removeEventListener("keydown", escape);
    };
  }, [open]);

  return (
    <div ref={root} className={`ui-ring-control ${ringTone(percent)}`}>
      <Button
        type="button"
        variant="bare"
        className={`ui-ring-trigger${open ? " is-open" : ""}${busy ? " is-busy" : ""}`}
        aria-label={accessible}
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        title={accessible}
        onClick={() => {
          setOpen((value) => !value);
          if (!open) onOpen?.();
        }}
      >
        <svg className="ui-ring" viewBox="0 0 20 20" aria-hidden="true">
          <circle className="ui-ring-track" cx="10" cy="10" r="7.5" />
          <circle
            className="ui-ring-fill"
            cx="10"
            cy="10"
            r="7.5"
            pathLength="100"
            strokeDasharray={`${percent ?? 0} 100`}
          />
        </svg>
      </Button>
      <span className="ui-ring-percent" aria-hidden="true">
        {display}
      </span>
      {open && (
        <section ref={panel} id={panelId} className="ui-ring-panel" role="region" aria-label={panelLabel} style={placement ? { ...placement, bottom: "auto" } : undefined}>
          <header>
            <span>上下文已用<b>{display}</b></span>
            {valueText && <strong>{valueText}</strong>}
          </header>
          {percent != null && (
            <div className="ui-ring-meter" aria-hidden="true">
              {displaySegments.length > 0 && displaySegments.every((segment) => segment.percent != null)
                ? displaySegments.map((segment) => <i key={segment.id} className={`is-${segment.id}`} style={{ width: `${Math.min(100, Math.max(0, segment.percent!))}%` }} />)
                : <i className="is-used" style={{ width: `${Math.min(100, Math.max(0, percent))}%` }} />}
            </div>
          )}
          {displaySegments.length > 0 && (
            <ul className="ui-ring-legend">
              {displaySegments.map((segment) => (
                <li key={segment.id} className={`is-${segment.id}`}>
                  <i aria-hidden="true" />
                  <span>{segment.label}</span>
                  <strong>{segment.valueText ?? (segment.percent == null ? "—" : `${Number(segment.percent.toFixed(1))}%`)}</strong>
                </li>
              ))}
            </ul>
          )}
          {children}
        </section>
      )}
    </div>
  );
};
