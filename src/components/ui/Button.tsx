import React from "react";
import clsx from "clsx";

export interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  /**
   * `bare` opts out of the button box entirely: no height, padding, radius,
   * background, shadow or glass. It keeps only the interaction semantics so a
   * row, a toast action or a thumbnail can be a real <button> without being
   * forced into the 26–38px control shell. All other variants are boxed.
   */
  variant?: "primary" | "secondary" | "danger" | "success" | "subtle" | "surface" | "bare";
  /** Ignored by `bare`, which has no fixed height. */
  size?: "xs" | "sm" | "md" | "lg";
  loading?: boolean;
  icon?: React.ReactNode;
  allowClickWhileLoading?: boolean;
}

const VARIANT_CLASS: Record<NonNullable<ButtonProps["variant"]>, string> = {
  primary: "btn-primary",
  secondary: "btn-secondary",
  danger: "btn-danger",
  success: "btn-success",
  subtle: "btn-subtle",
  surface: "btn-surface",
  bare: "btn-bare",
};

const SIZE_CLASS: Record<NonNullable<ButtonProps["size"]>, string> = {
  xs: "btn-xs",
  sm: "btn-sm",
  md: "btn-md",
  lg: "btn-lg",
};

export const Button: React.FC<ButtonProps> = ({
  children,
  variant = "secondary",
  size = "md",
  loading = false,
  icon,
  className,
  disabled,
  allowClickWhileLoading = false,
  style,
  ...props
}) => {
  const bare = variant === "bare";
  const iconOnly = !children && Boolean(icon);

  return (
    <button
      className={clsx(
        "btn",
        VARIANT_CLASS[variant],
        !bare && SIZE_CLASS[size],
        iconOnly && "btn-icon-only",
        loading && "btn-loading",
        className
      )}
      disabled={disabled || (loading && !allowClickWhileLoading)}
      style={loading && allowClickWhileLoading ? { ...style, pointerEvents: "auto" } : style}
      {...props}
    >
      {loading && <span className="btn-spinner" />}
      {!loading && icon && <span className="btn-icon">{icon}</span>}
      {children}
    </button>
  );
};

export default Button;
