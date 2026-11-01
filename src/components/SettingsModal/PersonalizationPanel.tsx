import React from "react";
import { TerminalThemeSettings } from "./TerminalThemeSettings";
import { SettingsGroup, SettingsPane } from "./settingsUI";

/** 个性化：终端主题、调色板与对比度。 */
export const PersonalizationPanel: React.FC = () => (
  <SettingsPane title="个性化">
    <SettingsGroup title="终端配色">
      <TerminalThemeSettings />
    </SettingsGroup>
  </SettingsPane>
);

export default PersonalizationPanel;
