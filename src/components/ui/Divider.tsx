import React from "react";
import clsx from "clsx";

export interface DividerProps {
  orientation?: "horizontal" | "vertical";
  /** Tighter margin, for dividers between dense demo rows. */
  dense?: boolean;
  dashed?: boolean;
  className?: string;
  style?: React.CSSProperties;
}

export const Divider: React.FC<DividerProps> = ({
  orientation = "horizontal",
  dense = false,
  dashed = false,
  className,
  style,
}) => {
  return (
    <div
      className={clsx(
        orientation === "horizontal" ? "divider-h" : "divider-v",
        dense && "dense",
        dashed && "dashed",
        className
      )}
      style={style}
      role="separator"
    />
  );
};

export default Divider;
