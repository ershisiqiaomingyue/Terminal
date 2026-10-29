import React from "react";
import { LoaderCircle } from "lucide-react";

/**
 * Spinner
 * ---------------------------------------------------------------------------
 * The one rotating "working" glyph of the design system. Views used to import
 * `LoaderCircle` from lucide and bolt a locally defined `.inline-spin` /
 * `.ui-compaction-spin` / `.chat-background-spinner` class onto it, which left
 * three copies of the same keyframes with three different durations. This
 * component owns the geometry, the rotation, the reduced-motion fallback and
 * the colour inheritance, so callers only choose a size.
 *
 * Colour comes from `currentColor` on purpose: the same spinner must read
 * neutral inside a status line and accent inside a compaction strip.
 */

export type SpinnerProps = {
  /** Glyph size in px; the stroke box matches the icon grid (default 14). */
  size?: number;
  strokeWidth?: number;
  /**
   * Accessible name. Omit when the spinner sits next to visible progress text,
   * where it is decoration and gets `aria-hidden`.
   */
  label?: string;
  className?: string;
};

export const Spinner: React.FC<SpinnerProps> = ({ size = 14, strokeWidth, label, className }) => (
  <LoaderCircle
    size={size}
    strokeWidth={strokeWidth}
    className={["ui-spinner", className].filter(Boolean).join(" ")}
    role={label ? "img" : undefined}
    aria-label={label}
    aria-hidden={label ? undefined : true}
  />
);

export default Spinner;
