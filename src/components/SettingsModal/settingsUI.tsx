import * as React from "react";
import { cn, UICard } from "rain-ui";

/**
 * 设置页基座组件 —— 零自定义 CSS，全部由 rain-ui 组件 + Tailwind 工具类构成：
 * - SettingsPane    右侧内容列（居中限宽 + 页面大标题）
 * - SettingsGroup   分组（小标题 + 一张卡片）
 * - SettingsCard    卡片容器（细描边、16px 圆角；行间发丝线由相邻行自动绘制）
 * - SettingRow      设置行（左侧标题+描述，右侧控件）
 * - SettingsActions 卡片底部操作条
 */

/** 右侧内容列：居中限宽 + 大标题；页面级操作与标题同一行。 */
export const SettingsPane: React.FC<{ title: string; children: React.ReactNode; action?: React.ReactNode }> = ({
  title,
  children,
  action,
}) => (
  <section
    data-settings-pane
    className="mx-auto w-full max-w-[728px] pb-16"
    style={{ fontFamily: '"Noto Sans SC", var(--font-sans)' }}
  >
    <div className="mb-9 flex min-h-8 items-center justify-between gap-6">
      <h2 data-settings-title className="text-[20px] font-semibold leading-[1.3] tracking-normal text-foreground">
        {title}
      </h2>
      {action ? <div data-settings-header-action className="shrink-0">{action}</div> : null}
    </div>
    {children}
  </section>
);

/** 分组：可选小标题 + 一张卡片。分组之间 44px。 */
export const SettingsGroup: React.FC<{ title?: string; children: React.ReactNode; className?: string }> = ({
  title,
  children,
  className,
}) => (
  <section className={cn("mb-10", className)}>
    {title ? <h3 className="mb-3.5 text-[12.5px] font-medium leading-[1.45] tracking-normal text-foreground">{title}</h3> : null}
    <SettingsCard>{children}</SettingsCard>
  </section>
);

/** 卡片：细描边 + 16px 圆角 + 无阴影；行与行之间画一条左右内缩的发丝线。 */
export const SettingsCard: React.FC<{ children: React.ReactNode; className?: string }> = ({ children, className }) => (
  <UICard
    data-settings-card
    className={cn(
      "rounded-2xl border-border/80 bg-card text-card-foreground shadow-none",
      "[&_.settings-row+.settings-row]:before:absolute",
      "[&_.settings-row+.settings-row]:before:inset-x-[17px]",
      "[&_.settings-row+.settings-row]:before:top-0",
      "[&_.settings-row+.settings-row]:before:h-px",
      "[&_.settings-row+.settings-row]:before:content-['']",
      "[&_.settings-row+.settings-row]:before:bg-border",
      className,
    )}
  >
    {children}
  </UICard>
);

/** 设置行：左侧标题+描述占满剩余宽度，右侧控件靠右对齐。 */
export const SettingRow: React.FC<{
  title: React.ReactNode;
  desc?: React.ReactNode;
  children?: React.ReactNode;
  /** 右侧控件区的附加类（控制宽度、纵向堆叠等）。 */
  controlClassName?: string;
  className?: string;
}> = ({ title, desc, children, controlClassName, className }) => (
  <div data-setting-row className={cn("settings-row relative grid min-h-[60px] grid-cols-[minmax(0,1fr)_auto] items-center gap-8 px-[17px] py-2.5", className)}>
    <div data-setting-copy className="min-w-0">
      <div data-setting-title className="text-[13px] font-medium leading-[1.45] tracking-normal text-foreground">{title}</div>
      {desc ? <div className="mt-[2px] text-[12.5px] font-normal leading-[1.5] tracking-normal text-[#737373]">{desc}</div> : null}
    </div>
    {children ? (
      <div data-setting-controls className={cn("flex min-w-0 items-center justify-end justify-self-end gap-2", controlClassName)}>{children}</div>
    ) : null}
  </div>
);

/** 卡片底部操作条（保存/测试连接等）。 */
export const SettingsActions: React.FC<{ children: React.ReactNode; className?: string }> = ({ children, className }) => (
  <div className={cn("flex items-center justify-end gap-2.5 border-t border-border px-[17px] py-3", className)}>
    {children}
  </div>
);



