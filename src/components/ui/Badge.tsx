import React from "react";
import clsx from "clsx";

export interface BadgeProps {
  variant?: "primary" | "success" | "warning" | "danger" | "neutral";
  pulse?: boolean;
  children: React.ReactNode;
  className?: string;
  /** 原生 title：给只能靠悬停看到细节的胶囊（如顶栏内核/权限）用。 */
  title?: string;
}

export const Badge: React.FC<BadgeProps> = ({
  variant = "neutral",
  pulse = false,
  children,
  className,
  title,
}) => {
  const variantClass = {
    primary: "badge-primary",
    success: "badge-success",
    warning: "badge-warning",
    danger: "badge-danger",
    neutral: "badge-neutral",
  }[variant];

  return (
    <span className={clsx("badge", variantClass, className)} title={title}>
      <span className={clsx("badge-dot", pulse && "badge-dot-pulse")} />
      {children}
    </span>
  );
};

export default Badge;
