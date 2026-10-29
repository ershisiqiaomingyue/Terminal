import React, { useEffect, useMemo, useState } from "react";
import { Save } from "lucide-react";
import { Button, Input } from "@/components/ui";
import { formatInvokeError, invoke, isTauri } from "@/platform/runtime";
import { useSettingsNotification } from "./SettingsNotification";

/**
 * P6-B UI：等待时长的数据面。
 *
 * 后端把 `~/.android-tool/agent-settings.json` 的 `permissionTimeoutSec` /
 * `questionTimeoutSec` 读进进程内缓存（缺省 300 / 600 秒，1–3600 钳位）。
 * P6-C 起 `0` 表示「永不超时」——权限条只能由用户决策或停止回合收尾。
 * P6-D 把两档拆开：权限是「要不要放行」，提问是「等人读题作答」。
 * 这里保存后后端会立刻刷新覆盖值，不必重启应用。
 */
interface AgentRuntimeSettings {
  permissionTimeoutSec: number;
  questionTimeoutSec: number;
  defaultPermissionTimeoutSec: number;
  defaultQuestionTimeoutSec: number;
  maxPermissionTimeoutSec: number;
  settingsPath: string;
}

interface TimeoutField {
  key: "permissionTimeoutSec" | "questionTimeoutSec";
  title: string;
  description: string;
  ariaLabel: string;
  fallbackDefault: number;
  /** 常用档位；P6-D 的提问档偏长。 */
  presets: number[];
}

const FIELDS: TimeoutField[] = [
  {
    key: "permissionTimeoutSec",
    title: "工具权限等待时长",
    description: "工具调用等待你授权的最长时间。到点自动拒绝并让回合继续；填 0 表示永不超时。",
    ariaLabel: "工具权限等待时长（秒）",
    fallbackDefault: 300,
    presets: [0, 60, 300, 600, 1800],
  },
  {
    key: "questionTimeoutSec",
    title: "提问等待时长",
    description: "ask_user 提问等你作答的最长时间。到点按跳过处理并让回合继续；填 0 表示永不超时。",
    ariaLabel: "提问等待时长（秒）",
    fallbackDefault: 600,
    presets: [0, 300, 600, 1800, 3600],
  },
];

function formatSeconds(seconds: number): string {
  if (seconds === 0) return "永不超时";
  if (seconds % 3600 === 0) return `${seconds / 3600} 小时`;
  if (seconds % 60 === 0) return `${seconds / 60} 分钟`;
  return `${seconds} 秒`;
}

export function AgentRuntimeSettings() {
  const notify = useSettingsNotification();
  const [settings, setSettings] = useState<AgentRuntimeSettings | null>(null);
  const [drafts, setDrafts] = useState<Record<string, string>>({
    permissionTimeoutSec: "300",
    questionTimeoutSec: "600",
  });
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    void invoke<AgentRuntimeSettings>("get_agent_settings")
      .then((value) => {
        if (cancelled) return;
        setSettings(value);
        setDrafts({
          permissionTimeoutSec: String(value.permissionTimeoutSec),
          questionTimeoutSec: String(value.questionTimeoutSec),
        });
      })
      .catch((error) => {
        if (!cancelled) notify(formatInvokeError(error), "error");
      });
    return () => { cancelled = true; };
  }, [notify]);

  const max = settings?.maxPermissionTimeoutSec ?? 3600;
  const parsed = useMemo(() => {
    const next: Record<string, number> = {};
    for (const field of FIELDS) {
      const value = Number(drafts[field.key]);
      next[field.key] = Number.isFinite(value) ? Math.trunc(value) : Number.NaN;
    }
    return next;
  }, [drafts]);

  const valid = (key: TimeoutField["key"]) => {
    const value = parsed[key];
    return Number.isFinite(value) && value >= 0 && value <= max;
  };
  const dirty = settings
    ? FIELDS.some((field) => parsed[field.key] !== settings[field.key])
    : false;

  const save = async () => {
    if (saving || !FIELDS.every((field) => valid(field.key))) return;
    setSaving(true);
    try {
      const value = await invoke<AgentRuntimeSettings>("update_agent_settings", {
        permissionTimeoutSec: parsed.permissionTimeoutSec,
        questionTimeoutSec: parsed.questionTimeoutSec,
      });
      setSettings((current) => (current ? { ...current, ...value } : current));
      setDrafts({
        permissionTimeoutSec: String(value.permissionTimeoutSec),
        questionTimeoutSec: String(value.questionTimeoutSec),
      });
      notify("等待时长已保存，立即生效。", "success");
    } catch (error) {
      notify(formatInvokeError(error), "error");
    } finally {
      setSaving(false);
    }
  };

  return (
    <div>
      <div className="setting-group-title">对话运行时</div>
      <div className="setting-card-group">
        {FIELDS.map((field) => (
          <div className="setting-row-item" key={field.key}>
            <div className="setting-label-block">
              <span className="setting-title">{field.title}</span>
              <span className="setting-desc">{field.description}</span>
            </div>
            <div className="setting-control-block">
              <Input
                size="sm"
                type="number"
                min={0}
                max={max}
                aria-label={field.ariaLabel}
                value={drafts[field.key]}
                onChange={(event) =>
                  setDrafts((current) => ({ ...current, [field.key]: event.target.value }))
                }
              />
              <Button
                size="sm"
                icon={<Save size={14} />}
                loading={saving}
                disabled={!dirty || !FIELDS.every((item) => valid(item.key))}
                onClick={() => void save()}
              >
                保存
              </Button>
            </div>
            <div className="setting-label-block">
              <span className="setting-desc">
                {field.presets.map((value) => (
                  <Button
                    key={value}
                    size="sm"
                    variant={parsed[field.key] === value ? "primary" : "subtle"}
                    onClick={() =>
                      setDrafts((current) => ({ ...current, [field.key]: String(value) }))
                    }
                  >
                    {formatSeconds(value)}
                  </Button>
                ))}
              </span>
            </div>
          </div>
        ))}
        <div className="setting-row-item">
          <div className="setting-label-block">
            <span className="setting-title">取值范围</span>
            <span className="setting-desc">
              {settings
                ? `0–${max} 秒；缺省权限 ${formatSeconds(settings.defaultPermissionTimeoutSec)}、提问 ${formatSeconds(settings.defaultQuestionTimeoutSec)}。`
                : "正在读取当前设置…"}
            </span>
          </div>
        </div>
      </div>
    </div>
  );
}
