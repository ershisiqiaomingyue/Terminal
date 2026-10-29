import { createElement, type HTMLAttributes } from "react";
import { X } from "lucide-react";
import { Button, type ButtonProps } from "@/components/ui/Button";
import { Input, type InputProps } from "@/components/ui/Input";

/** Shared treatment for composer tasks, queued messages and user interactions. */
export function ChatNoticeSurface({ as = "div", className = "", ...props }: HTMLAttributes<HTMLElement> & { as?: "div" | "section" }) {
  return createElement(as, { ...props, className: `chat-notice-surface ${className}` });
}

export function ChatNoticeAction({ className = "", variant = "subtle", children, ...props }: ButtonProps) {
  return <Button type="button" size="xs" variant={variant}
    className={`chat-notice-action${children == null ? " is-icon" : ""}${variant === "primary" ? " is-emphasized" : ""} ${className}`} {...props}>{children}</Button>;
}

export function ChatNoticeClose({ label, ...props }: Omit<ButtonProps, "children" | "icon" | "aria-label"> & { label: string }) {
  return <ChatNoticeAction title={label} aria-label={label} icon={<X size={15} />} {...props} />;
}

/** The same single-ring spinner used by library buttons, with no icon fill. */
export function ChatNoticeSpinner() {
  return <span className="btn-spinner chat-notice-spinner" aria-hidden="true" />;
}

export function ChatNoticeInput({ wrapperClassName = "", ...props }: InputProps) {
  return <Input size="sm" wrapperClassName={`chat-notice-input ${wrapperClassName}`} {...props} />;
}
