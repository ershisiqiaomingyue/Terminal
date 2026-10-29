import React, { useEffect, useId, useRef, useState } from "react";
import { ChevronRight } from "lucide-react";
import { Button } from "./Button";

/**
 * NoticePanel
 * ---------------------------------------------------------------------------
 * Shared notice used by both the component library and the chat composer.
 *
 * Six kinds, all click-to-expand except the inline queue:
 *   permission / question – approval and clarification
 *   plan                  – 任务编排 (task orchestration)
 *   background            – 后台任务
 *   queue                 – 追加消息 (inline, never collapses)
 *   files                 – 文件改动 (compact, +N −N summary, no action buttons)
 *
 * Two details that are easy to get wrong:
 *
 *   1. There is no hover-to-expand, and no click-outside-to-close. Every
 *      collapsible row expands on click; a second click on the same row (or
 *      Escape) collapses it. Clicking elsewhere never closes it.
 *   2. 文件改动 carries no 撤销 / 查看变更 buttons. Those live on the settled
 *      file-change card in the AI reply, not on this in-progress row.
 */

export type NoticeKind = "permission" | "question" | "plan" | "background" | "queue" | "files";

export type NoticeTone = "neutral" | "attention" | "warning" | "danger" | "success";

export type NoticePanelProps = {
  label: string;
  summary: React.ReactNode;
  /** Custom inline content replacing `summary` (e.g. coloured +N −N). */
  summaryContent?: React.ReactNode;
  icon?: React.ReactNode;
  /** Right-aligned counter. For 文件改动 the product reads "N 个文件". */
  count?: number;
  countSuffix?: string;
  /** Inline meta after the summary, e.g. the 任务编排 elapsed pill. */
  meta?: React.ReactNode;
  badge?: React.ReactNode;
  actions?: React.ReactNode;
  tone?: NoticeTone;
  /** Right-angle chevron, used by 文件改动. */
  compact?: boolean;
  /** Inline body: no collapsible header, content always shown. */
  inline?: boolean;
  /**
   * Inline 卡片也能折叠（默认不折叠）。只有多条追加消息用得到：列表长了要能收起来，
   * 单条胶囊展开/收起没有意义。
   */
  collapsible?: boolean;
  /** Opens expanded. */
  defaultExpanded?: boolean;
  expanded?: boolean;
  onExpandedChange?: (expanded: boolean) => void;
  /** Fired on expand, e.g. to open a planner popover. Never expands by hover. */
  onActivate?: () => void;
  children?: React.ReactNode;
};

export const NoticePanel: React.FC<NoticePanelProps> = ({
  label,
  summary,
  summaryContent,
  icon,
  count,
  countSuffix,
  meta,
  badge,
  actions,
  tone = "neutral",
  compact = false,
  inline = false,
  collapsible = false,
  defaultExpanded = false,
  expanded: controlled,
  onExpandedChange,
  onActivate,
  children,
}) => {
  const [uncontrolled, setUncontrolled] = useState(defaultExpanded);
  const expanded = controlled ?? uncontrolled;
  const contentId = useId();
  const root = useRef<HTMLElement>(null);
  // 「展开/收起」的箭头方向统一给所有 variant：收起朝右（>）、展开朝下（v）。
  // 形状永远是直角箭头，展开态靠 CSS `rotate(90deg)` 转成下箭头，这样在两种
  // 尺寸（compact / 普通）下转轴和留白都一致，不会左右跳。
  const Chevron = ChevronRight;

  useEffect(() => {
    if (!expanded) return;
    const close = () => {
      if (controlled === undefined) setUncontrolled(false);
      onExpandedChange?.(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      event.preventDefault();
      root.current?.querySelector<HTMLButtonElement>('[aria-expanded="true"]')?.focus({ preventScroll: true });
      close();
    };
    // 只有 Escape 会收起：**点击卡片外侧不收起**。这些卡片都是用户主动点开的，
    // 点一下对话区域就把它们关掉会让人反复重开；收起统一靠再次点击同一行。
    document.addEventListener("keydown", escape);
    return () => document.removeEventListener("keydown", escape);
  }, [expanded, controlled, onExpandedChange]);

  const toggle = () => {
    const next = !expanded;
    if (controlled === undefined) setUncontrolled(next);
    onExpandedChange?.(next);
    if (next) onActivate?.();
  };

  const classes = [
    "ui-notice",
    `is-${tone}`,
    compact && "is-compact",
    inline && "is-inline",
    expanded && "is-expanded",
  ]
    .filter(Boolean)
    .join(" ");

  if (inline) {
    // 内联卡片（追加消息）默认不折叠，但**照样有卡片头和卡片外壳**——它和其它四种
    // 通知并排出现，缺一个头就差一档视觉层级。`collapsible` 打开后头部才变成
    // 真正的展开/收起按钮（多条追加消息用得到）；即使不折叠，抬头样式也与其它卡
    // 完全一致（用 `.ui-notice-summary.is-static` 而不是另起一套 `.ui-notice-heading`），
    // 只是不带箭头、不可点。
    // 传空串 label 时（追加消息卡：抬头文字由 summaryContent 自己给）不再渲染
    // `.ui-notice-label`，否则 label 和 summaryContent 会并排显示成两遍。
    const heading = (
      <>
        {icon && (
          <span className="ui-notice-icon" aria-hidden="true">
            {icon}
          </span>
        )}
        {label !== "" && <span className="ui-notice-label">{label}</span>}
        <span className="ui-notice-preview">{summaryContent ?? summary}</span>
        {count != null && (
          <small className="ui-notice-count">
            {count}
            {countSuffix ? ` ${countSuffix}` : ""}
          </small>
        )}
        {meta && <span className="ui-notice-meta">{meta}</span>}
        {badge}
        {actions && <div className="ui-notice-actions">{actions}</div>}
      </>
    );
    return (
      <section className={classes} aria-label={label || undefined}>
        <div className="ui-notice-heading">
          {collapsible ? (
            <Button
              type="button"
              variant="bare"
              className="ui-notice-summary is-static"
              aria-label={label || undefined}
              aria-expanded={expanded}
              aria-controls={expanded ? contentId : undefined}
              onClick={toggle}
            >
              {heading}
              <Chevron size={14} className="ui-notice-chevron" aria-hidden="true" />
            </Button>
          ) : (
            <div className="ui-notice-summary is-static">{heading}</div>
          )}
        </div>
        {/* 不可折叠（单条追加消息）时没有箭头可点，children 必须一直渲染——否则卡片
            只剩图标和边框，看起来是「空卡」。可折叠时仍由展开态门控。 */}
        {(expanded || !collapsible) && <div id={contentId}>{children}</div>}
      </section>
    );
  }

  return (
    <section ref={root} className={classes} aria-label={label}>
      <div className="ui-notice-heading">
        <Button
          type="button"
          variant="bare"
          className="ui-notice-summary"
          aria-label={label}
          aria-expanded={expanded}
          aria-controls={expanded ? contentId : undefined}
          onClick={toggle}
        >
          {icon && (
            <span className="ui-notice-icon" aria-hidden="true">
              {icon}
            </span>
          )}
          <span className="ui-notice-label">{label}</span>
          <span className="ui-notice-preview">{summaryContent ?? summary}</span>
          {count != null && (
            <small className="ui-notice-count">
              {count}
              {countSuffix ? ` ${countSuffix}` : ""}
            </small>
          )}
          {meta && <span className="ui-notice-meta">{meta}</span>}
          {badge}
          <Chevron size={14} className="ui-notice-chevron" aria-hidden="true" />
        </Button>
        {actions && <div className="ui-notice-actions">{actions}</div>}
      </div>
      {expanded && (
        <div id={contentId} className="ui-notice-content">
          {children}
        </div>
      )}
    </section>
  );
};
