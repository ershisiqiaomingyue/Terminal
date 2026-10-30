import React from "react";
import { Card, CardBody } from "rain-ui";
import { AgentRuntimeSettings } from "./AgentRuntimeSettings";
import { AgentThinkingSettings } from "./AgentThinkingSettings";

/** Agent：组合对话运行时（等待时长）与 AI 状态文案两组设置。 */
export const AgentAssistPanel: React.FC = () => {
  return (
    <section className="settings-pane">
      <header className="pane-header">
        <h2 className="pane-title">Agent</h2>
      </header>

      <Card tone="surface" className="settings-rain-card">
        <CardBody>
          <AgentRuntimeSettings />
        </CardBody>
      </Card>

      <Card tone="surface" className="settings-rain-card">
        <CardBody>
          <AgentThinkingSettings />
        </CardBody>
      </Card>
    </section>
  );
};

export default AgentAssistPanel;
