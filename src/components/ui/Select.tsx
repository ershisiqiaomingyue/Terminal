import React, { useState, useRef, useEffect, useCallback, useId } from "react";
import { createPortal } from "react-dom";
import clsx from "clsx";
import { ChevronDown, Check, Search, Trash2 } from "lucide-react";
import { Input } from "./Input";

export interface SelectOptionAction {
  icon: React.ReactNode;
  label: string;
  onClick: () => void;
  danger?: boolean;
}

export interface SelectOption {
  label: string;
  value: string;
  onRemove?: () => void;
  removeLabel?: string;
  group?: { id: string; label: string };
  /** Secondary line under the label, e.g. provider or endpoint hint. */
  description?: string;
  /** Renders the row dimmed and cannot be selected. */
  disabled?: boolean;
  /**
   * Compact right-aligned hint rendered on the same line as the label, e.g. a
   * context-window badge. Keeps the row single-line instead of stacking a
   * second description line.
   */
  trailing?: string;
  /**
   * Leading visual rendered on the same line before the label, e.g. a row of
   * theme color dots. Keeps the row single-line; label stays the search text.
   */
  leading?: React.ReactNode;
  /** Trailing controls that must not select the row. */
  actions?: SelectOptionAction[];
  /**
   * Opens a nested panel anchored to this row, so a menu can host a second
   * level of choice without closing the outer select.
   */
  submenu?: (close: () => void) => React.ReactNode;
}

export interface SelectProps {
  options: SelectOption[];
  value?: string;
  defaultValue?: string;
  onChange?: (val: string) => void;
  size?: "sm" | "md";
  placeholder?: string;
  disabled?: boolean;
  searchable?: boolean;
  searchPlaceholder?: string;
  className?: string;
  style?: React.CSSProperties;
  /** Rendered above the option list. */
  header?: React.ReactNode;
  /** Rendered below the option list. */
  footer?: React.ReactNode;
  /** Keeps the menu open after selecting a row. */
  keepOpenOnSelect?: boolean;
  "aria-label"?: string;
}

export const Select: React.FC<SelectProps> = ({
  options,
  value,
  defaultValue,
  onChange,
  size = "md",
  placeholder = "请选择...",
  disabled = false,
  searchable = false,
  searchPlaceholder = "搜索选项",
  header,
  footer,
  keepOpenOnSelect = false,
  className,
  style,
  "aria-label": ariaLabel,
}) => {
  const [internalVal, setInternalVal] = useState(value ?? defaultValue ?? options[0]?.value);
  const [isOpen, setIsOpen] = useState(false);
  const [search, setSearch] = useState("");
  const [openSubmenu, setOpenSubmenu] = useState<string | null>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const triggerId = useId();
  const dropdownRef = useRef<HTMLDivElement>(null);
  const unavailable = disabled || options.length === 0;
  const expanded = isOpen && !unavailable;

  const currentVal = value !== undefined ? value : internalVal;
  const currentOption = options.find((opt) => opt.value === currentVal);
  const query = search.trim().toLowerCase();
  const visibleOptions = searchable && query
    ? options.filter((option) => option.label.toLowerCase().includes(query) || option.value.toLowerCase().includes(query) || option.group?.label.toLowerCase().includes(query))
    : options;

  useEffect(() => {
    if (unavailable) setIsOpen(false);
  }, [unavailable]);

  const [coords, setCoords] = useState<{
    top: number;
    left?: number;
    right?: number;
    minWidth: number;
    openUp: boolean;
    listMaxHeight: number;
    menuMaxHeight: number;
  }>({
    top: 0,
    minWidth: 200,
    openUp: false,
    listMaxHeight: 240,
    menuMaxHeight: 264,
  });

  const updatePosition = useCallback(() => {
    if (!triggerRef.current) return;
    const rect = triggerRef.current.getBoundingClientRect();
    const viewportHeight = window.innerHeight;
    const viewportWidth = window.innerWidth;

    // 预估菜单高度并判断是否朝上弹出
    const estimatedHeight = Math.min(options.length * 36 + 16, 260) + (searchable ? 42 : 0);
    const spaceBelow = Math.max(0, viewportHeight - rect.bottom - 14);
    const spaceAbove = Math.max(0, rect.top - 14);
    const openUp = spaceBelow < estimatedHeight && spaceAbove > spaceBelow;
    const available = openUp ? spaceAbove : spaceBelow;

    const minWidth = Math.min(viewportWidth - 16, Math.max(rect.width, 220));
    const top = openUp ? rect.top - 6 : rect.bottom + 6;
    const chromeHeight = 12
      + (dropdownRef.current?.querySelector<HTMLElement>(".select-menu-header")?.offsetHeight || 0)
      + (dropdownRef.current?.querySelector<HTMLElement>(".select-search")?.offsetHeight || 0)
      + (dropdownRef.current?.querySelector<HTMLElement>(".select-menu-footer")?.offsetHeight || 0);
    const menuMaxHeight = Math.max(1, available);
    const listMaxHeight = Math.max(1, Math.min(240, available - chromeHeight));

    // 靠右检测：如果向右扩展超出屏幕，改为靠右对齐
    if (rect.left + minWidth > viewportWidth - 16) {
      setCoords({
        top,
        right: Math.max(16, viewportWidth - rect.right),
        minWidth,
        openUp,
        listMaxHeight,
        menuMaxHeight,
      });
    } else {
      setCoords({
        top,
        left: Math.max(12, rect.left),
        minWidth,
        openUp,
        listMaxHeight,
        menuMaxHeight,
      });
    }
  }, [options.length, searchable]);

  useEffect(() => {
    if (!expanded) return;
    updatePosition();

    const handleScrollOrResize = () => {
      updatePosition();
    };

    const handleOutsideClick = (e: PointerEvent) => {
      const target = e.target as Node;
      if (
        triggerRef.current &&
        !triggerRef.current.contains(target) &&
        dropdownRef.current &&
        !dropdownRef.current.contains(target)
      ) {
        setIsOpen(false);
      }
    };
    const handleEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopImmediatePropagation();
      setIsOpen(false);
      triggerRef.current?.focus();
    };

    // 捕获阶段监听任何滚动容器与窗口变化
    window.addEventListener("scroll", handleScrollOrResize, true);
    window.addEventListener("resize", handleScrollOrResize);
    document.addEventListener("pointerdown", handleOutsideClick, true);
    document.addEventListener("keydown", handleEscape, true);

    return () => {
      window.removeEventListener("scroll", handleScrollOrResize, true);
      window.removeEventListener("resize", handleScrollOrResize);
      document.removeEventListener("pointerdown", handleOutsideClick, true);
      document.removeEventListener("keydown", handleEscape, true);
    };
  }, [expanded, updatePosition, searchable]);

  const handleSelect = (val: string) => {
    if (unavailable) return;
    setInternalVal(val);
    onChange?.(val);
    if (!keepOpenOnSelect) setIsOpen(false);
    if (searchable) triggerRef.current?.focus();
  };

  const dropdownContent = expanded && typeof document !== "undefined" ? (
    createPortal(
      <div
        ref={dropdownRef}
        data-select-trigger={triggerId}
        className="select-dropdown select-glass-menu is-portaled"
        onKeyDown={(event) => {
          if (!searchable) return;
          if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            setIsOpen(false);
            triggerRef.current?.focus();
          } else if (event.key === "ArrowDown" && event.target instanceof HTMLInputElement) {
            event.preventDefault();
            dropdownRef.current?.querySelector<HTMLElement>('[role="option"]')?.focus();
          } else if (event.key === "Enter" && event.target instanceof HTMLInputElement) {
            event.preventDefault();
            event.stopPropagation();
            if (visibleOptions.length) handleSelect(visibleOptions[0].value);
          }
        }}
        style={{
          position: "fixed",
          top: coords.openUp ? "auto" : `${coords.top}px`,
          bottom: coords.openUp ? `${window.innerHeight - coords.top}px` : "auto",
          left: coords.left !== undefined ? `${coords.left}px` : "auto",
          right: coords.right !== undefined ? `${coords.right}px` : "auto",
          minWidth: `${coords.minWidth}px`,
          maxWidth: "min(560px, calc(100vw - 24px))",
          width: "max-content",
          zIndex: 999999,
          display: "block",
          maxHeight: `${coords.menuMaxHeight}px`,
          overflowY: "auto",
          "--select-list-max-height": `${coords.listMaxHeight}px`,
        } as React.CSSProperties}
      >
        {header && <div className="select-menu-header">{header}</div>}
        {searchable && <div className="select-search">
          <Input size="sm" type="search" value={search} onChange={(event) => setSearch(event.target.value)} placeholder={searchPlaceholder} aria-label={searchPlaceholder} prefixIcon={<Search size={14} />} autoFocus />
        </div>}
        <div className="select-options-list menu-options-list" role="listbox">
          {visibleOptions.map((opt, index) => {
            const isSelected = opt.value === currentVal;
            const isSubmenuOpen = openSubmenu === opt.value;
            return (
              <React.Fragment key={opt.value}>
              {opt.group && opt.group.id !== visibleOptions[index - 1]?.group?.id && <div className="select-group-label" role="presentation">{opt.group.label}</div>}
              <div className={clsx("select-option-row", opt.group && "is-grouped", isSubmenuOpen && "is-submenu-open")} role="presentation">
              <div
                className={clsx(
                  "select-option select-item-pill",
                  isSelected && "selected is-active",
                  opt.disabled && "is-disabled",
                  opt.description && "has-description"
                )}
                role="option"
                aria-selected={isSelected}
                aria-disabled={opt.disabled || undefined}
                tabIndex={opt.disabled ? -1 : 0}
                title={opt.label}
                onClick={() => { if (!opt.disabled) handleSelect(opt.value); }}
                onKeyDown={(event) => {
                  if (opt.disabled) return;
                  if (event.key === "Enter" || event.key === " ") {
                    event.preventDefault();
                    event.stopPropagation();
                    handleSelect(opt.value);
                  } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                    event.preventDefault();
                    const items = Array.from(dropdownRef.current?.querySelectorAll<HTMLElement>('[role="option"]') || []);
                    const next = items[items.indexOf(event.currentTarget) + (event.key === "ArrowDown" ? 1 : -1)];
                    if (next) next.focus();
                    else if (event.key === "ArrowUp") dropdownRef.current?.querySelector<HTMLInputElement>("input")?.focus();
                  }
                }}
              >
                {opt.leading && (
                  <span className="select-option-leading" style={{ flexShrink: 0 }} title={opt.label}>
                    {opt.leading}
                  </span>
                )}
                <span className="select-option-copy">
                  <span className="select-option-text">{opt.label}</span>
                  {opt.description && <span className="select-option-description">{opt.description}</span>}
                </span>
                {opt.trailing && (
                  <span
                    className="select-option-trailing"
                    style={{ flexShrink: 0, marginLeft: "auto" }}
                    title={opt.trailing}
                  >
                    {opt.trailing}
                  </span>
                )}
                {isSelected && !opt.submenu && (
                  <Check
                    size={14}
                    style={{ color: "inherit", flexShrink: 0, marginLeft: opt.trailing ? 6 : "auto" }}
                  />
                )}
              </div>
              {opt.actions?.map((action) => (
                <button
                  key={action.label}
                  type="button"
                  className={clsx("select-option-action", action.danger && "is-danger")}
                  aria-label={action.label}
                  title={action.label}
                  onClick={(event) => {
                    event.stopPropagation();
                    action.onClick();
                  }}
                >{action.icon}</button>
              ))}
              {opt.onRemove && <button type="button" className="select-option-remove" aria-label={opt.removeLabel || `删除 ${opt.label}`} title={opt.removeLabel || `删除 ${opt.label}`} onClick={(event) => {
                event.stopPropagation();
                opt.onRemove?.();
                triggerRef.current?.focus();
              }}><Trash2 size={13} /></button>}
              {opt.submenu && <button type="button" className={clsx("select-option-expand", isSubmenuOpen && "is-open")} aria-label={`展开 ${opt.label}`} aria-expanded={isSubmenuOpen} title={`展开 ${opt.label}`} onClick={(event) => {
                event.stopPropagation();
                setOpenSubmenu(isSubmenuOpen ? null : opt.value);
              }}><ChevronDown size={13} /></button>}
              {isSubmenuOpen && opt.submenu && <div className="select-submenu" role="dialog" aria-label={opt.label} onClick={(event) => event.stopPropagation()}>
                {opt.submenu(() => setOpenSubmenu(null))}
              </div>}
              </div>
              </React.Fragment>
            );
          })}
        </div>
        {searchable && visibleOptions.length === 0 && <div className="select-search-empty" role="status">未找到匹配模型或选项</div>}
        {footer && <div className="select-menu-footer">{footer}</div>}
      </div>,
      document.body
    )
  ) : null;

  return (
    <div
      className={clsx(
        "select-pod modern-select",
        `select-${size}`,
        expanded && "is-open",
        className
      )}
      style={{ position: "relative", width: "100%", ...style }}
    >
      <button
        id={triggerId}
        ref={triggerRef}
        type="button"
        className="select-trigger select-trigger-bar"
        aria-haspopup="listbox"
        aria-label={ariaLabel}
        aria-expanded={expanded}
        disabled={unavailable}
        title={currentOption?.label || currentVal || placeholder}
        onClick={() => {
          if (unavailable) return;
          setSearch("");
          setIsOpen(!isOpen);
        }}
      >
        <span className="select-value-text select-echo-box">
          {currentOption?.leading}
          {currentOption ? currentOption.label : (currentVal || placeholder)}
        </span>
        <span className="select-arrow select-chevron-icon">
          <ChevronDown size={14} />
        </span>
      </button>

      {dropdownContent}
    </div>
  );
};

export default Select;
