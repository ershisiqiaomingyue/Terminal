import { HOME_TOOL_GROUPS, homeToolStatus } from "@/views/Home";
import type { SupportedToolKey } from "@/views/ToolWindows";

interface ContextNavigationProps {
  pageType: "home" | "tool" | "components" | "code";
  activeTool?: SupportedToolKey;
  tabs: readonly { id: string; title: string; type: string }[];
  activeId: string;
  onSelect: (id: string) => void;
  onOpenTool: (tool: SupportedToolKey) => void;
  onNavigate: () => void;
}

/** Mirrors the numbered sections in views/Components, in page order. */
const COMPONENT_SECTIONS = [
  { id: "01", label: "01 · Card 卡片配色" },
  { id: "02", label: "02 · Button 按钮" },
  { id: "03", label: "03 · 表单原子" },
  { id: "04", label: "04 · Select 选择器" },
  { id: "05", label: "05 · Table 数据表格" },
  { id: "06", label: "06 · Badge 状态灯" },
  { id: "07", label: "07 · TaskStepList 步骤" },
  { id: "08", label: "08 · Toast 轻提示" },
  { id: "09", label: "09 · NoticePanel 通知面板" },
  { id: "10", label: "10 · Attachment 附件" },
  { id: "11", label: "11 · ProgressRing 环形进度" },
  { id: "12", label: "12 · UsageMetrics 用量指标" },
  { id: "13", label: "13 · FilePreviewPanel 文件预览" },
  { id: "14", label: "14 · FileChangeCard 回复后改动" },
];

export function ContextNavigation({ pageType, activeTool, tabs, activeId, onSelect, onOpenTool, onNavigate }: ContextNavigationProps) {
  const title = pageType === "components" ? "组件库" : pageType === "code" ? "代码查看" : "工具";
  return <>
    <header className="context-nav-head"><h2>{title}</h2></header>
    <nav className="context-nav-scroll" aria-label={`${title}列表`}>
      {pageType === "code" ? <section className="context-nav-section">
        <h3 className="context-nav-section-label">已打开</h3>
        {tabs.filter((tab) => tab.type === "code").map((tab) => <button key={tab.id} type="button" className={`context-nav-row${tab.id === activeId ? " is-active" : ""}`} aria-current={tab.id === activeId ? "page" : undefined} onClick={() => onSelect(tab.id)}><span>{tab.title}</span></button>)}
      </section> : pageType === "components" ? <section className="context-nav-section">
        <h3 className="context-nav-section-label">组件</h3>
        {COMPONENT_SECTIONS.map((section) => <button key={section.id} type="button" className="context-nav-row" onClick={() => {
          // Every Section renders id={`component-section-<NN>`}, so no heading-text
          // matching is involved. scrollIntoView is avoided on purpose: it walks
          // every scrollable ancestor, and .app-main clips with overflow:hidden,
          // which can swallow the request. Scroll the real container explicitly.
          const scroller = document.querySelector<HTMLElement>(".app-viewport");
          const target = document.getElementById(`component-section-${section.id}`);
          if (scroller && target) {
            const top =
              target.getBoundingClientRect().top -
              scroller.getBoundingClientRect().top +
              scroller.scrollTop -
              12;
            scroller.scrollTo({ top, behavior: "smooth" });
          }
          onNavigate();
        }}><span>{section.label}</span></button>)}
      </section> : HOME_TOOL_GROUPS.map((group) => <section className="context-nav-section context-nav-tool-group" key={group.title} aria-label={group.title}>
        <h3 className="context-nav-section-label">{group.title}</h3>
        {group.tools.filter((tool) => tool.toolKey && homeToolStatus(tool) === "ready").map((tool) => <button key={tool.id} type="button" className={`context-nav-row${tool.toolKey === activeTool ? " is-active" : ""}`} title={tool.title} aria-current={tool.toolKey === activeTool ? "page" : undefined} onClick={() => tool.toolKey && onOpenTool(tool.toolKey)}><span className="context-nav-tool-icon" aria-hidden="true">{tool.icon}</span><span>{tool.title}</span></button>)}
      </section>)}
    </nav>
  </>;
}
