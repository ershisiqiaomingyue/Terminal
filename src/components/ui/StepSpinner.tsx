import React from "react";
import { LoaderCircle } from "lucide-react";

/**
 * StepSpinner —— TaskStepList（组件 07）运行态的「旋转虚线弧」加载图标：
 * 主色淡底圆 + 旋转弧线。抽成独立组件供各处加载态复用（用户指定口径）。
 */
export type StepSpinnerProps = {
  /** 外圈直径（默认 20，与任务步骤圆一致）。 */
  size?: number;
  label?: string;
};

export const StepSpinner: React.FC<StepSpinnerProps> = ({ size = 20, label }) => (
  <span
    className="ui-step-spinner"
    style={{ width: size, height: size }}
    role={label ? "img" : undefined}
    aria-label={label}
    aria-hidden={label ? undefined : true}
  >
    <LoaderCircle size={Math.round(size * 0.65)} strokeWidth={2.6} />
  </span>
);

export default StepSpinner;
