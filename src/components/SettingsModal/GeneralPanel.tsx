import React from "react";
import { Select } from "rain-ui";
import {
  APP_FONT_OPTIONS,
  APP_MONO_FONT_OPTIONS,
  readAppFont,
  readAppMonoFont,
  setAppFont,
  setAppMonoFont,
  type AppFont,
  type AppMonoFont,
} from "@/platform/font";
import { FileTransferSettings } from "./FileTransferSettings";
import { SettingsPane, SettingsGroup, SettingRow } from "./settingsUI";

export const GeneralPanel: React.FC = () => {
  const [appLanguage, setAppLanguage] = React.useState("zh-CN");
  const [appFont, setFontSelection] = React.useState<AppFont>(readAppFont);
  const [appMonoFont, setMonoFontSelection] = React.useState<AppMonoFont>(readAppMonoFont);

  return (
    <SettingsPane title="常规">
      <SettingsGroup title="界面首选项">
        <FileTransferSettings />

        <SettingRow title="界面语言 (Language)" desc="应用操作界面显示语言" controlClassName="justify-end">
          <Select
            size="md"
            value={appLanguage}
            onChange={(val) => setAppLanguage(val as string)}
            options={[
              { label: "简体中文", value: "zh-CN" },
              { label: "English (US)", value: "en-US" },
            ]}
          />
        </SettingRow>

        <SettingRow title="界面字体" desc="设置应用界面使用的字体。" controlClassName="justify-end">
          <Select
            size="md"
            value={appFont}
            onChange={(value) => {
              const font = value as AppFont;
              setAppFont(font);
              setFontSelection(font);
            }}
            options={[...APP_FONT_OPTIONS]}
            searchable
            searchPlaceholder="搜索界面字体"
          />
        </SettingRow>

        <SettingRow title="代码与终端字体" desc="设置代码与终端使用的字体。" controlClassName="justify-end">
          <Select
            size="md"
            value={appMonoFont}
            onChange={(value) => {
              const font = value as AppMonoFont;
              setAppMonoFont(font);
              setMonoFontSelection(font);
            }}
            options={[...APP_MONO_FONT_OPTIONS]}
            searchable
            searchPlaceholder="搜索代码字体"
          />
        </SettingRow>
      </SettingsGroup>
    </SettingsPane>
  );
};
