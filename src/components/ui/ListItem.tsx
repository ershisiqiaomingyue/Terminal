import React, { useId, useState } from "react";
import { ChevronRight } from "lucide-react";
import { Button } from "./Button";

/**
 * ListItem
 * ---------------------------------------------------------------------------
 * The repeating row used by the skills list, provider list, repository list and
 * most settings groups: a text column on the left, an optional action cluster
 * on the right, an optional detail area underneath.
 *
 * Slots, top to bottom:
 *   leading  – icon or avatar before the text column
 *   title    – bold line
 *   meta     – inline pills after the title (source tags, counts)
 *   description – secondary paragraph, wraps
 *   footer   – third line: status text, command, timestamps
 *   actions  – right-hand cluster: switch, icon buttons
 *   extra    – full-width detail area; only rendered when expanded
 *
 * Interaction
 *   The text area is a disclosure button and hover covers the inset row.
 *   Actions remain siblings of that button so a switch never expands it.
 */

export type ListItemProps = {
  leading?: React.ReactNode;
  title: React.ReactNode;
  meta?: React.ReactNode;
  description?: React.ReactNode;
  footer?: React.ReactNode;
  actions?: React.ReactNode;
  /** Detail area. Present but collapsed unless `expandable` is set. */
  extra?: React.ReactNode;
  /** Turns the row into a disclosure: clicking anywhere toggles `extra`. */
  expandable?: boolean;
  /** Starts expanded when `expandable` is set. */
  defaultExpanded?: boolean;
  expanded?: boolean;
  onExpandedChange?: (expanded: boolean) => void;
  showChevron?: boolean;
  ariaLabel?: string;
  /** Hover tint. Off for static display rows that are not clickable. */
  interactive?: boolean;
  /** Divider under the row. The last row in a group usually drops it. */
  divider?: boolean;
  /** Stacks the action cluster under the text column on narrow widths. */
  compact?: boolean;
  /** Tighter vertical rhythm, for dense panels. */
  dense?: boolean;
  /** Selected / active tint, for the row matching the current view. */
  selected?: boolean;
  onClick?: () => void;
  className?: string;
};

export const ListItem: React.FC<ListItemProps> = ({
  leading,
  title,
  meta,
  description,
  footer,
  actions,
  extra,
  expandable = false,
  defaultExpanded = false,
  expanded: controlledExpanded,
  onExpandedChange,
  showChevron = true,
  ariaLabel,
  interactive = true,
  divider = true,
  compact = false,
  dense = false,
  selected = false,
  onClick,
  className,
}) => {
  const [uncontrolledExpanded, setExpanded] = useState(defaultExpanded);
  const expanded = controlledExpanded ?? uncontrolledExpanded;
  const detailsId = useId();
  const hasExtra = Boolean(extra);
  const canExpand = expandable && hasExtra;
  const isClickable = canExpand || Boolean(onClick);

  const toggle = () => {
    if (canExpand) {
      if (controlledExpanded === undefined) setExpanded(!expanded);
      onExpandedChange?.(!expanded);
    }
    onClick?.();
  };

  const classes = [
    "ui-list-item",
    interactive && "is-interactive",
    !divider && "no-divider",
    compact && "is-compact",
    dense && "is-dense",
    selected && "is-selected",
    canExpand && "is-expandable",
    canExpand && expanded && "is-expanded",
    className,
  ]
    .filter(Boolean)
    .join(" ");

  const content = (
    <>
      {canExpand && showChevron && (
        <span className="ui-list-item-chevron" aria-hidden="true">
          <ChevronRight size={14} />
        </span>
      )}
      {leading && <span className="ui-list-item-leading">{leading}</span>}
      <div className="ui-list-item-body">
        <div className="ui-list-item-head">
          <span className="ui-list-item-title">{title}</span>
          {meta && <span className="ui-list-item-meta">{meta}</span>}
        </div>
        {description && <div className="ui-list-item-desc">{description}</div>}
        {footer && <div className="ui-list-item-footer">{footer}</div>}
      </div>
    </>
  );

  return (
    <div className={classes}>
      <div className="ui-list-item-row" onClick={(event) => { if (isClickable && event.target === event.currentTarget) toggle(); }}>
        {isClickable ? <Button type="button" variant="bare" className="ui-list-item-hit" onClick={toggle}
          aria-label={ariaLabel ?? (typeof title === "string" ? title : undefined)}
          aria-expanded={canExpand ? expanded : undefined} aria-controls={canExpand && expanded ? detailsId : undefined}>
          {content}
        </Button> : <div className="ui-list-item-static">{content}</div>}
        {actions && <div className="ui-list-item-actions" onClick={(event) => event.stopPropagation()} role="presentation">
          {actions}
        </div>}
      </div>
      {canExpand && expanded && <div id={detailsId} className="ui-list-item-extra">{extra}</div>}
    </div>
  );
};
