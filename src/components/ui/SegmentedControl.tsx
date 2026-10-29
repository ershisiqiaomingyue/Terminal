import React from "react";
import clsx from "clsx";
import { Button } from "./Button";

export interface SegmentedControlProps<T extends string> {
  value: T;
  onChange: (value: T) => void;
  options: readonly { value: T; label: React.ReactNode; disabled?: boolean }[];
  label: string;
  className?: string;
  buttonClassName?: string;
}

/** One shared interaction and palette for view tabs and segmented filters. */
export function SegmentedControl<T extends string>({ value, onChange, options, label, className, buttonClassName }: SegmentedControlProps<T>) {
  return (
    <div className={clsx("segmented-glass", className)} role="group" aria-label={label}>
      {options.map((option) => (
        <Button key={option.value} type="button" variant="bare" className={clsx("seg-btn", buttonClassName, value === option.value && "active")}
          aria-pressed={value === option.value} disabled={option.disabled} onClick={() => onChange(option.value)}>
          {option.label}
        </Button>
      ))}
    </div>
  );
}
