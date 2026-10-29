import { ToolBackButton } from "@/components/ui/ToolBackButton";
import { useToolWindowValue } from "@/platform/toolWindowState";
import React, { useEffect, useState } from "react";
import {
  Power,
  RotateCcw,
  Volume2,
  Volume1,
  VolumeX,
  Sun,
  Moon,
  Home,
  ArrowLeft,
  Layers,
  Trash2,
  FileText,
  RotateCw,
  RefreshCw,
  ShieldAlert,
} from "lucide-react";
import { Button, Badge, Select } from "@/components/ui";
import { useConfirmAction } from "@/components/ConfirmDialog/useConfirmAction";
import {
  deviceLabel,
  deviceOptionLabel,
  formatInvokeError,
  invoke,
  isTauri,
  listAdbDevices,
  saveFile,
  type AdbDevice,
} from "@/platform/runtime";
import { useApplicationPageAction } from "@/application/useApplicationPageAction";
import { notify, publishAiTaskActivity, notifyDesktopRequired } from "@/platform/notifications";
import "./device-control.css";

export const DeviceControlView: React.FC<{ onBack?: () => void }> = ({ onBack }) => {
  const { confirmAction, confirmationDialog } = useConfirmAction();
  const tauri = isTauri();
  const [devices, setDevices] = useState<AdbDevice[]>([]);
  const [serial, setSerial] = useToolWindowValue("control", "serial", "");

  const showToast = (msg: string) => {
    notify(msg, "info");
  };

  const online = devices.filter((item) => item.state === "device");
  const selected = online.find((item) => item.serial === serial);
  const deviceText = (selected ? deviceLabel(selected) : "未连接");
  const noDevice = !tauri || !serial;

  const refreshDevices = async () => {
    if (!tauri) {
      notifyDesktopRequired();
      return;
    }
    try {
      const list = await listAdbDevices();
      const nextOnline = list.filter((item) => item.state === "device");
      setDevices(nextOnline);
      if (nextOnline.length === 0) {
        setSerial("");
        showToast("未检测到在线设备");
        return false;
      }
      setSerial((current) =>
        nextOnline.some((item) => item.serial === current) ? current : nextOnline[0].serial
      );
    } catch (reason) {
      setDevices([]);
      setSerial("");
      notify(formatInvokeError(reason), "error");
      return false;
    }
  };

  useEffect(() => {
    if (!tauri) return;
    void refreshDevices();
  }, [tauri]);

  const handleAction = async (name: string, desc: string) => {
    if (!tauri) {
      notifyDesktopRequired();
      return;
    }
    if (!serial) {
      showToast("未检测到在线设备");
      return;
    }
    try {
      const result = await invoke<string>("run_adb_device_action", { serial, action: name });
      showToast(result.trim() ? result : "指令已执行");
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    }
  };

  const executeReboot = async (mode: "normal" | "recovery" | "bootloader") => {
    if (!tauri) {
      notifyDesktopRequired();
      return;
    }
    if (!serial) {
      showToast("未检测到在线设备");
      return;
    }
    try {
      await invoke("restart_adb_device", { serial, mode });
      const done = {
        normal: "已发送正常重启指令",
        recovery: "已发送 Recovery 重启指令",
        bootloader: "已发送 Bootloader 重启指令",
      }[mode];
      showToast(done);
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    }
  };

  const handleReboot = (mode: "normal" | "recovery" | "bootloader") => confirmAction({
    title: "重启设备", confirmLabel: "确认重启",
    message: `确认重启设备 ${serial || deviceText}${mode === "normal" ? "" : mode === "recovery" ? "至 Recovery" : "至 Bootloader"}？未保存的数据可能丢失。`,
    onConfirm: () => executeReboot(mode),
  });

  const handleBugreport = async () => {
    if (!tauri) {
      notifyDesktopRequired();
      return;
    }
    if (!serial) {
      showToast("未检测到在线设备");
      return;
    }
    try {
      const destination = await saveFile("保存 Bugreport", `bugreport-${serial}.zip`, [
        { name: "Bugreport", extensions: ["zip"] },
      ]);
      if (!destination) return;
      const taskId = await invoke<string>("start_adb_bugreport", {
        request: { serial, destination },
      });
      showToast(taskId);
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    }
  };

  useApplicationPageAction("open.tool.control", (action) => {
    if (!tauri) { notifyDesktopRequired(action.actionId); return; }
    if (action.parameters.serial) setSerial(action.parameters.serial);
    const detail = action.parameters.operation
      ? `已选择调试动作 ${action.parameters.operation}；页面会保留确认步骤。`
      : "已刷新设备连接；调试动作需要在页面中确认。";
    publishAiTaskActivity({ running: true, status: "running", title: "正在准备设备控制", detail, actionId: action.actionId, progress: 20 });
    void refreshDevices()
      .then((result) => publishAiTaskActivity({ running: false, status: result === false ? "error" : "done", title: result === false ? "设备控制等待连接" : "设备控制已就绪", detail: result === false ? "未检测到在线设备或连接查询失败。" : detail, actionId: action.actionId, progress: result === false ? null : 100 }))
      .catch((reason) => publishAiTaskActivity({ running: false, status: "error", title: "设备控制初始化失败", detail: formatInvokeError(reason), actionId: action.actionId }));
  });

  return (
    <div className="device-control-container">
      {confirmationDialog}
      <header className="device-control-header">
        <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <ToolBackButton onBack={onBack} />
          <h2 style={{ margin: 0, fontSize: 15, fontWeight: 700, color: "var(--text-primary)" }}>
            设备快捷控制与系统调试
          </h2>
          <Badge variant="primary">{deviceText}</Badge>
          {tauri && (
            <div style={{ width: 220, flex: "0 0 220px" }}>
              <Select
                size="sm"
                value={serial}
                placeholder="未检测到在线设备"
                disabled={online.length === 0}
                options={online.map((item) => ({ label: deviceOptionLabel(item), value: item.serial }))}
                onChange={setSerial}
              />
            </div>
          )}
        </div>

        <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <Button
            variant="secondary"
            size="sm"
            icon={<RefreshCw size={13} />}
            onClick={() => void refreshDevices()}
          >
            刷新连接
          </Button>
        </div>
      </header>

      <main className="device-control-content custom-scrollbar">
        <div className="device-control-grid">
          {/* 1. 屏幕与电源控制 */}
          <div className="device-control-card">
            <div className="device-control-card-head">
              <span className="device-control-card-title">
                <Power size={15} color="var(--text-secondary)" />
                屏幕与按键模拟
              </span>
              <Badge variant="neutral">Input Key</Badge>
            </div>
            <p style={{ margin: 0, fontSize: 12, color: "var(--text-tertiary)" }}>
              通过 input keyevent 向系统分发底层硬件按键指令。
            </p>
            <div className="device-control-buttons-row">
              <Button variant="secondary" size="sm" icon={<Sun size={13} />} disabled={noDevice} onClick={() => void handleAction("wake", "唤醒屏幕 (KEYCODE_WAKEUP)")}>
                唤醒屏幕
              </Button>
              <Button variant="secondary" size="sm" icon={<Moon size={13} />} disabled={noDevice} onClick={() => void handleAction("sleep", "休眠屏幕 (KEYCODE_SLEEP)")}>
                熄灭屏幕
              </Button>
              <Button variant="secondary" size="sm" icon={<Power size={13} />} disabled={noDevice} onClick={() => void handleAction("power", "电源键 (KEYCODE_POWER)")}>
                电源键
              </Button>
            </div>
          </div>

          {/* 2. 系统全局导航 */}
          <div className="device-control-card">
            <div className="device-control-card-head">
              <span className="device-control-card-title">
                <Home size={15} color="var(--text-secondary)" />
                虚拟导航动作
              </span>
              <Badge variant="neutral">Navigation</Badge>
            </div>
            <p style={{ margin: 0, fontSize: 12, color: "var(--text-tertiary)" }}>
              触发主屏幕、返回以及多任务后台近期应用切换。
            </p>
            <div className="device-control-buttons-row">
              <Button variant="secondary" size="sm" icon={<Home size={13} />} disabled={noDevice} onClick={() => void handleAction("home", "返回桌面 (KEYCODE_HOME)")}>
                主屏幕
              </Button>
              <Button variant="secondary" size="sm" icon={<ArrowLeft size={13} />} disabled={noDevice} onClick={() => void handleAction("back", "返回上一级 (KEYCODE_BACK)")}>
                返回
              </Button>
              <Button variant="secondary" size="sm" icon={<Layers size={13} />} disabled={noDevice} onClick={() => void handleAction("app-switch", "近期任务 (KEYCODE_APP_SWITCH)")}>
                多任务后台
              </Button>
            </div>
          </div>

          {/* 3. 音量与媒体控制 */}
          <div className="device-control-card">
            <div className="device-control-card-head">
              <span className="device-control-card-title">
                <Volume2 size={15} color="var(--text-secondary)" />
                媒体音量调节
              </span>
              <Badge variant="neutral">Audio</Badge>
            </div>
            <p style={{ margin: 0, fontSize: 12, color: "var(--text-tertiary)" }}>
              调节设备当前主音轨与通知流音频电平。
            </p>
            <div className="device-control-buttons-row">
              <Button variant="secondary" size="sm" icon={<Volume2 size={13} />} disabled={noDevice} onClick={() => void handleAction("vol-up", "增加音量 (KEYCODE_VOLUME_UP)")}>
                增加音量
              </Button>
              <Button variant="secondary" size="sm" icon={<Volume1 size={13} />} disabled={noDevice} onClick={() => void handleAction("vol-down", "降低音量 (KEYCODE_VOLUME_DOWN)")}>
                降低音量
              </Button>
              <Button variant="secondary" size="sm" icon={<VolumeX size={13} />} disabled={noDevice} onClick={() => void handleAction("vol-mute", "全局静音 (KEYCODE_VOLUME_MUTE)")}>
                静音
              </Button>
            </div>
          </div>

          {/* 4. 维护与诊断 */}
          <div className="device-control-card">
            <div className="device-control-card-head">
              <span className="device-control-card-title">
                <Trash2 size={15} color="var(--text-secondary)" />
                快速诊断运维
              </span>
              <Badge variant="neutral">Diagnostics</Badge>
            </div>
            <p style={{ margin: 0, fontSize: 12, color: "var(--text-tertiary)" }}>
              清空系统 Logcat 环形缓冲区或提取全量 Bugreport 日志包。
            </p>
            <div className="device-control-buttons-row">
              <Button variant="secondary" size="sm" icon={<Trash2 size={13} />} disabled={noDevice} onClick={() => confirmAction({ title: "清空设备日志", message: `确认清空设备 ${serial || deviceText} 的 Logcat 环形缓冲区？未保存的日志无法恢复。`, confirmLabel: "确认清空", onConfirm: () => handleAction("clear-logcat", "清空 Logcat 环形缓冲区 (adb logcat -c)") })}>
                清空 Logcat
              </Button>
              <Button variant="secondary" size="sm" icon={<RotateCw size={13} />} disabled={noDevice} onClick={() => void handleAction("rotate", "顺时针旋转屏幕 90°")}>
                旋转屏幕
              </Button>
              <Button variant="secondary" size="sm" icon={<FileText size={13} />} disabled={noDevice} onClick={() => void handleBugreport()}>
                导出 Bugreport
              </Button>
            </div>
          </div>
        </div>

        {/* 5. 危险启动与模式切换 */}
        <div className="device-control-card" style={{ marginTop: 8 }}>
          <div className="device-control-card-head">
            <span className="device-control-card-title">
              <ShieldAlert size={15} color="var(--danger-text)" />
              高级重启模式控制
            </span>
            <Badge variant="warning">系统级危险操作</Badge>
          </div>
          <p style={{ margin: 0, fontSize: 12, color: "var(--text-tertiary)" }}>
            发送 adb reboot 命令切换设备启动模式，请确保所有修改已落盘。
          </p>
          <div className="device-control-buttons-row">
            <Button
              variant="danger"
              size="sm"
              icon={<RotateCcw size={13} />}
              disabled={noDevice}
              onClick={() => void handleReboot("normal")}
            >
              正常重启设备
            </Button>
            <Button
              variant="danger"
              size="sm"
              disabled={noDevice}
              onClick={() => void handleReboot("recovery")}
            >
              进入 Recovery 模式
            </Button>
            <Button
              variant="danger"
              size="sm"
              disabled={noDevice}
              onClick={() => void handleReboot("bootloader")}
            >
              进入 Bootloader (Fastboot)
            </Button>
          </div>
        </div>
      </main>
    </div>
  );
};

export default DeviceControlView;
