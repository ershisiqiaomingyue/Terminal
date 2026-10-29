import React from "react";
import brandIcon from "@/assets/brand-icon.svg";
import { version } from "../../../package.json";
import { DiagnosticLogsPanel } from "./DiagnosticLogsPanel";
import { Checkbox } from "@/components/ui/Checkbox";
import { Button } from "@/components/ui/Button";
import { readComponentDebugTargets, setComponentDebugTarget } from "@/platform/componentDebugPreferences";

export const AboutPanel: React.FC = () => {
  const [debugTargets, setDebugTargets] = React.useState(readComponentDebugTargets);
  const [debugOpen, setDebugOpen] = React.useState(false);
  const updateDebugTarget = (target: "chat" | "components", enabled: boolean) => {
    setComponentDebugTarget(target, enabled);
    setDebugTargets((current) => ({ ...current, [target]: enabled }));
  };

  return (
    <section className="settings-pane">
      <header className="pane-header">
        <h2 className="pane-title">关于 Android Toolkit</h2>
      </header>

      <div>
        <div className="setting-group-title">基础设施信息</div>
        <div className="setting-card-group about-card">
          <div className="setting-row-item">
            <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
              <img src={brandIcon} width={48} height={48} alt="Android Toolkit 图标" draggable={false} style={{ flexShrink: 0 }} />
              <div className="setting-label-block">
                <span className="setting-title">Android Toolkit</span>
                <span className="setting-desc">开发者工具箱</span>
              </div>
            </div>
            <span style={{ fontSize: 13, fontWeight: 700, color: "var(--accent-primary)" }}>
              v{version}
            </span>
          </div>

          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">技术栈</span>
              <span className="setting-desc">React 18 · TypeScript · Vite · Tauri 2 · Rust</span>
            </div>
          </div>
        </div>
      </div>
      <div>
        <div className="setting-group-title">组件调试</div>
        <div className="setting-card-group">
          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">启用调试入口</span>
              <span className="setting-desc">选择显示组件调试入口的页面</span>
            </div>
            <Button size="sm" aria-expanded={debugOpen} aria-controls="about-debug-targets" onClick={() => setDebugOpen((open) => !open)}>调试</Button>
          </div>
          {debugOpen && <div id="about-debug-targets" className="setting-row-item" role="group" aria-label="调试页面">
            <div className="settings-preference-actions" style={{ gap: 14 }}>
              <Checkbox checked={debugTargets.chat} label="对话页" onChange={(checked) => updateDebugTarget("chat", checked)} />
              <Checkbox checked={debugTargets.components} label="组件库" onChange={(checked) => updateDebugTarget("components", checked)} />
            </div>
          </div>}
        </div>
      </div>
      <DiagnosticLogsPanel />
    </section>
  );
};
