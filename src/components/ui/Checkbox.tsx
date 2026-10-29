import React, { useEffect, useRef } from "react";
import clsx from "clsx";

export interface CheckboxProps extends Omit<React.InputHTMLAttributes<HTMLInputElement>, "type" | "onChange"> {
  label?: React.ReactNode;
  indeterminate?: boolean;
  onChange?: (checked: boolean) => void;
}

export function Checkbox({ label, indeterminate = false, onChange, className, ...props }: CheckboxProps) {
  const inputRef = useRef<HTMLInputElement>(null);
  useEffect(() => { if (inputRef.current) inputRef.current.indeterminate = indeterminate; }, [indeterminate]);
  const input = <input {...props} ref={inputRef} type="checkbox" className={clsx("checkbox", className)}
    aria-checked={indeterminate ? "mixed" : props.checked} onChange={(event) => onChange?.(event.target.checked)} />;
  return label ? <label className="checkbox-row">{input}<span>{label}</span></label> : input;
}
