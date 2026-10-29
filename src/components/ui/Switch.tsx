import React from "react";
import clsx from "clsx";

export interface SwitchProps {
  checked?: boolean;
  defaultChecked?: boolean;
  onChange?: (checked: boolean) => void;
  disabled?: boolean;
  label?: string;
  className?: string;
  "aria-label"?: string;
}

export const Switch: React.FC<SwitchProps> = ({
  checked,
  defaultChecked,
  onChange,
  disabled = false,
  label,
  className,
  "aria-label": ariaLabel,
}) => {
  return (
    <label
      className={clsx(
        "switch-wrapper",
        disabled && "is-disabled",
        className
      )}
      style={{ display: "inline-flex", alignItems: "center", gap: 8, cursor: disabled ? "not-allowed" : "pointer" }}
    >
      <span className="switch">
        <input
          type="checkbox"
          aria-label={ariaLabel || label}
          checked={checked}
          defaultChecked={defaultChecked}
          onChange={(e) => onChange?.(e.target.checked)}
          disabled={disabled}
        />
        <span className="switch-slider" />
      </span>
      {label && <span className="switch-label" style={{ fontSize: 13 }}>{label}</span>}
    </label>
  );
};

export default Switch;
