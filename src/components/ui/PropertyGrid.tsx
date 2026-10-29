import React from "react";
import clsx from "clsx";

export interface PropertyItem {
  icon?: React.ReactNode;
  label: string;
  value: React.ReactNode;
  valueType?: "default" | "orange" | "green" | "accent" | "unknown" | "danger";
}

export interface PropertyGridProps {
  items: PropertyItem[];
  columns?: 1 | 2;
  className?: string;
  style?: React.CSSProperties;
}

export const PropertyGrid: React.FC<PropertyGridProps> = ({
  items,
  columns = 2,
  className,
  style,
}) => {
  // 如果是2列，将 items 平分成左右两半或按网格排列
  const half = Math.ceil(items.length / 2);
  const col1 = columns === 2 ? items.slice(0, half) : items;
  const col2 = columns === 2 ? items.slice(half) : [];

  const renderCol = (list: PropertyItem[]) => (
    <div className="property-grid-col">
      {list.map((item, idx) => {
        const pending = typeof item.value === "string" && /^(未检测|未验签|未发现|未知|尚未|等待|正在)/.test(item.value);
        const tone = pending ? "unknown" : item.valueType;
        return (
          <div key={idx} className="property-row">
            <div className="property-label-group">
              {item.icon && <span className="property-icon">{item.icon}</span>}
              <span className="property-name">{item.label}</span>
            </div>
            <span
              className={clsx(
                "property-value",
                tone === "orange" && "value-orange",
                tone === "green" && "value-green",
                tone === "accent" && "value-accent",
                tone === "unknown" && "value-unknown",
                tone === "danger" && "value-danger"
              )}
              title={typeof item.value === "string" ? item.value : undefined}
            >
              {item.value}
            </span>
          </div>
        );
      })}
    </div>
  );

  return (
    <div className={clsx("property-grid-card", className)} style={style}>
      <div className={clsx(columns === 2 && "property-grid-columns")}>
        {renderCol(col1)}
        {columns === 2 && renderCol(col2)}
      </div>
    </div>
  );
};

export default PropertyGrid;
