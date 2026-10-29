import React from "react";
import clsx from "clsx";
import { X } from "lucide-react";
import { Button } from "./Button";

export interface InputProps extends Omit<React.InputHTMLAttributes<HTMLInputElement>, "size"> {
  size?: "sm" | "md" | "lg";
  variant?: "sunken" | "surface";
  prefixIcon?: React.ReactNode;
  suffixAction?: React.ReactNode;
  clearable?: boolean;
  onClear?: () => void;
  wrapperClassName?: string;
  wrapperStyle?: React.CSSProperties;
  inputStyle?: React.CSSProperties;
}

export const Input = React.forwardRef<HTMLInputElement, InputProps>(
  (
    {
      size = "md",
      variant = "sunken",
      prefixIcon,
      suffixAction,
      clearable,
      onClear,
      className,
      style,
      wrapperClassName,
      wrapperStyle,
      inputStyle,
      value,
      ...props
    },
    ref
  ) => {
    const sizeClass = {
      sm: "input-sm",
      md: "input-md",
      lg: "input-lg",
    }[size];

    return (
      <div
        className={clsx(
          "input-shell input-wrapper",
          variant === "surface" && "input-surface",
          prefixIcon && "has-lead input-with-icon-prefix",
          (clearable || suffixAction) && "has-tail input-with-action",
          wrapperClassName,
          className
        )}
        style={{ ...style, ...wrapperStyle }}
      >
        {prefixIcon && <span className="input-lead-icon input-prefix-icon">{prefixIcon}</span>}
        <input
          ref={ref}
          className={clsx("desk-input input", sizeClass)}
          value={value}
          style={{ width: "100%", ...inputStyle }}
          {...props}
        />
        {clearable && value && (
          <Button
            type="button"
            variant="bare"
            className="input-clear-pill"
            icon={<X size={12} />}
            onClick={onClear}
            title="清空"
          />
        )}
        {suffixAction && <span className="input-action-wrap" style={{ marginLeft: 6 }}>{suffixAction}</span>}
      </div>
    );
  }
);

Input.displayName = "Input";
export default Input;
