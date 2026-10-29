import { ToolBackButton } from "@/components/ui/ToolBackButton";
import { useToolWindowValue } from "@/platform/toolWindowState";
import React, { useEffect, useRef, useState } from "react";
import {
  Camera,
  Video,
  Square,
  Play,
  Download,
  Copy,
  RefreshCw,
  Smartphone,
  Eye,
  CheckCircle2,
  Film,
} from "lucide-react";
import { Button, Badge, Select } from "@/components/ui";
import { PropertyGrid, type PropertyItem } from "@/components/ui/PropertyGrid";
import { ToolEmptyState } from "@/components/ui/ToolEmptyState";
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
import { notify, publishAiTaskActivity, notifyDesktopRequired } from "@/platform/notifications";
import { useApplicationPageAction } from "@/application/useApplicationPageAction";
import "./device-capture.css";

interface CaptureItem {
  id: string;
  name: string;
  type: "image" | "video";
  size: string;
  time: string;
  resolution: string;
  url?: string;
  path?: string;
}

function fileNameOf(path: string): string {
  return path.split(/[/\\]/).pop() || path;
}

export const DeviceCaptureView: React.FC<{ onBack?: () => void }> = ({ onBack }) => {
  const tauri = isTauri();
  const [devices, setDevices] = useState<AdbDevice[]>([]);
  const [serial, setSerial] = useToolWindowValue("capture", "serial", "");
  const [isRecording, setIsRecording] = useToolWindowValue("capture", "isRecording", false);
  const [recordSeconds, setRecordSeconds] = useToolWindowValue("capture", "recordSeconds", 0);
  const [gallery, setGallery] = useToolWindowValue<CaptureItem[]>("capture", "gallery", []);
  const [activeItem, setActiveItem] = useToolWindowValue<CaptureItem | null>("capture", "activeItem", null);
  const [captureInfo, setCaptureInfo] = useState<{ physicalSize?: string; physicalDensity?: string } | null>(null);

  const showToast = (msg: string) => {
    notify(msg, "info");
  };

  const online = devices.filter((item) => item.state === "device");
  const selected = online.find((item) => item.serial === serial);
  const device = (selected ? deviceLabel(selected) : "未连接");
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
        return;
      }
      setSerial((current) =>
        nextOnline.some((item) => item.serial === current) ? current : nextOnline[0].serial
      );
    } catch (reason) {
      setDevices([]);
      setSerial("");
      notify(formatInvokeError(reason), "error");
    }
  };

  useEffect(() => {
    if (!tauri) return;
    void refreshDevices();
  }, [tauri]);

  useEffect(() => {
    setCaptureInfo(null);
    if (!tauri || !serial) return;
    let cancelled = false;
    void invoke<{ physicalSize?: string; physicalDensity?: string }>("get_adb_capture_info", { serial })
      .then((value) => { if (!cancelled) setCaptureInfo(value); })
      .catch(() => { if (!cancelled) setCaptureInfo(null); });
    return () => { cancelled = true; };
  }, [tauri, serial]);

  const timerRef = useRef<number | null>(null);

  useEffect(() => {
    if (isRecording) {
      timerRef.current = window.setInterval(() => {
        setRecordSeconds((s) => s + 1);
      }, 1000);
    } else {
      if (timerRef.current) clearInterval(timerRef.current);
      setRecordSeconds(0);
    }
    return () => {
      if (timerRef.current) clearInterval(timerRef.current);
    };
  }, [isRecording]);

  const handleTakeScreenshot = async (serialOverride?: string, actionId?: string) => {
    const targetSerial = serialOverride || serial;
    if (!tauri) {
      notifyDesktopRequired(actionId);
      return;
    }
    if (!targetSerial) {
      const message = "未检测到在线设备";
      showToast(message);
      if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "设备截图未执行", detail: message, actionId });
      return;
    }
    try {
      const destination = await saveFile("保存截图", `screenshot-${targetSerial}.png`, [
        { name: "PNG", extensions: ["png"] },
      ]);
      if (!destination) return;
      const saved = await invoke<string>("capture_adb_screenshot", {
        request: { serial: targetSerial, remotePath: "/", localPath: destination },
      });
      const path = saved || destination;
      const item: CaptureItem = {
        id: `cap-${Date.now()}`,
        name: fileNameOf(path),
        type: "image",
        size: "",
        time: new Date().toLocaleTimeString(),
        resolution: "",
        path,
        url: path,
      };
      setGallery((current) => [item, ...current]);
      setActiveItem(item);
      showToast(path);
      if (actionId) publishAiTaskActivity({ running: false, status: "done", title: "设备截图已保存", detail: path, actionId, progress: 100 });
    } catch (reason) {
      const message = formatInvokeError(reason);
      notify(message, "error");
      if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "设备截图失败", detail: message, actionId });
    }
  };

  const handleToggleRecord = async (serialOverride?: string, actionId?: string) => {
    const targetSerial = serialOverride || serial;
    if (!tauri) {
      notifyDesktopRequired(actionId);
      return;
    }
    if (!targetSerial) {
      const message = "未检测到在线设备";
      showToast(message);
      if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "设备录屏未执行", detail: message, actionId });
      return;
    }
    if (!isRecording) {
      try {
        const taskId = await invoke<string>("start_adb_screen_record", { serial: targetSerial });
        setIsRecording(true);
        showToast(taskId);
        if (actionId) publishAiTaskActivity({ running: true, status: "running", title: "设备录屏进行中", detail: `已在 ${targetSerial} 启动录屏；停止后选择保存位置。`, actionId, progress: null });
      } catch (reason) {
        const message = formatInvokeError(reason);
        notify(message, "error");
        if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "设备录屏启动失败", detail: message, actionId });
      }
      return;
    }
    try {
      const destination = await saveFile("保存录屏", `screenrecord-${targetSerial}.mp4`, [
        { name: "MP4", extensions: ["mp4"] },
      ]);
      if (!destination) return;
      const saved = await invoke<string>("stop_adb_screen_record", {
        request: { serial: targetSerial, destination },
      });
      setIsRecording(false);
      const path = saved || destination;
      const item: CaptureItem = {
        id: `cap-${Date.now()}`,
        name: fileNameOf(path),
        type: "video",
        size: "",
        time: new Date().toLocaleTimeString(),
        resolution: "",
        path,
        url: path,
      };
      setGallery((current) => [item, ...current]);
      setActiveItem(item);
      showToast(path);
      if (actionId) publishAiTaskActivity({ running: false, status: "done", title: "设备录屏已保存", detail: path, actionId, progress: 100 });
    } catch (reason) {
      const message = formatInvokeError(reason);
      notify(message, "error");
      if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "设备录屏保存失败", detail: message, actionId });
    }
  };

  useApplicationPageAction("open.tool.capture", (action) => {
    const targetSerial = action.parameters.serial || serial;
    if (action.parameters.serial) setSerial(action.parameters.serial);
    const operation = action.parameters.operation || "screenshot";
    publishAiTaskActivity({ running: true, status: "running", title: operation === "record" ? "正在启动设备录屏" : "正在抓取设备截图", detail: "等待系统确认保存位置。", actionId: action.actionId, progress: 10 });
    if (operation === "record") {
      void handleToggleRecord(targetSerial, action.actionId);
    } else {
      void handleTakeScreenshot(targetSerial, action.actionId);
    }
  });

  const savedPath = activeItem?.path || activeItem?.url || "";

  const handleCopy = async () => {
    if (!tauri) {
      notifyDesktopRequired();
      return;
    }
    if (!savedPath) {
      showToast("没有可复制的路径");
      return;
    }
    try {
      await navigator.clipboard.writeText(savedPath);
      showToast(savedPath);
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    }
  };

  const handleExport = () => {
    if (!tauri) {
      notifyDesktopRequired();
      return;
    }
    showToast(savedPath || "没有可导出的本地文件");
  };

  const captureProperties: PropertyItem[] = [
    { icon: <Smartphone size={14} />, label: "当前选定设备", value: device },
    { icon: <Eye size={14} />, label: "物理屏幕分辨率", value: captureInfo?.physicalSize || "未检测", valueType: captureInfo?.physicalSize ? "default" : "unknown" },
    { icon: <Film size={14} />, label: "录屏最高帧率", value: "由设备 screenrecord 决定", valueType: "unknown" },
    { icon: <CheckCircle2 size={14} />, label: "截图保存格式", value: "PNG", valueType: "accent" },
  ];

  return (
    <div className="device-capture-container">

      <header className="device-capture-header">
        <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <ToolBackButton onBack={onBack} />
          <h2 style={{ margin: 0, fontSize: 15, fontWeight: 700, color: "var(--text-primary)" }}>
            设备截图与高清录屏
          </h2>
          <Badge variant="neutral">{noDevice ? "未连接" : "ADB 连接"}</Badge>
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
            刷新状态
          </Button>
        </div>
      </header>

      <div className="device-capture-body">
        {/* 左侧控制栏 */}
        <aside className="device-capture-controls-col custom-scrollbar">
          <PropertyGrid items={captureProperties} columns={1} />
          {noDevice && <ToolEmptyState compact title="等待连接设备" description="连接设备后才能截图或录屏。" />}

          {/* 屏幕截图动作 */}
          <div className="device-capture-action-card">
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
              <strong style={{ fontSize: 13, color: "var(--text-primary)" }}>屏幕快速截图</strong>
              <Camera size={16} color="var(--accent-primary)" />
            </div>
            <p style={{ margin: 0, fontSize: 11.5, color: "var(--text-tertiary)", lineHeight: 1.5 }}>
              以原生 Surface 缓冲区分辨率无损抓取并保存为 PNG 图像。
            </p>
            <Button
              variant="primary"
              size="sm"
              icon={<Camera size={13} />}
              disabled={noDevice}
              onClick={() => void handleTakeScreenshot()}
            >
              保存设备截图
            </Button>
          </div>

          {/* 屏幕录像动作 */}
          <div className="device-capture-action-card">
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
              <strong style={{ fontSize: 13, color: "var(--text-primary)" }}>屏幕视频录制</strong>
              <Video size={16} color="var(--danger-text)" />
            </div>
            <p style={{ margin: 0, fontSize: 11.5, color: "var(--text-tertiary)", lineHeight: 1.5 }}>
              基于底层 MediaCodec 硬件加速录制高清 MP4 视频流。
            </p>

            {isRecording && (
              <div style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 12, color: "var(--status-danger-text)", fontWeight: 600 }}>
                <span className="inline-block w-2.5 h-2.5 rounded-full bg-red-500 animate-ping" />
                <span>录制中：00:{recordSeconds < 10 ? `0${recordSeconds}` : recordSeconds} / 03:00</span>
              </div>
            )}

            <Button
              variant={isRecording ? "danger" : "secondary"}
              size="sm"
              icon={isRecording ? <Square size={13} /> : <Play size={13} />}
              disabled={noDevice}
              onClick={() => void handleToggleRecord()}
            >
              {isRecording ? "停止并保存 MP4" : "开始屏幕录制"}
            </Button>
          </div>
        </aside>

        {/* 右侧预览与画廊 */}
        <main className="device-capture-gallery-col custom-scrollbar">
          <div className="device-capture-preview-screen">
            {activeItem ? (
              <div style={{ textAlign: "center", padding: 20 }}>
                <div style={{ display: "inline-flex", padding: 16, borderRadius: 20, background: "var(--capture-item-bg)", marginBottom: 12 }}>
                  {activeItem.type === "video" ? <Film size={36} color="var(--accent-primary)" /> : <Camera size={36} color="var(--success-text)" />}
                </div>
                <h4 style={{ margin: "0 0 6px", fontSize: 14, color: "var(--text-primary)" }}>{activeItem.name}</h4>
                <p style={{ margin: 0, fontSize: 12, color: "var(--text-tertiary)" }}>
                  {[activeItem.resolution, activeItem.size, activeItem.time].filter(Boolean).join(" · ")}
                </p>
                <div style={{ display: "flex", gap: 8, justifyContent: "center", marginTop: 14 }}>
                  <Button variant="secondary" size="xs" icon={<Copy size={12} />} onClick={() => void handleCopy()}>
                    复制图像
                  </Button>
                  <Button variant="primary" size="xs" icon={<Download size={12} />} onClick={handleExport}>
                    导出到本地
                  </Button>
                </div>
              </div>
            ) : (
              <div style={{ textAlign: "center", padding: 20, color: "var(--text-tertiary)", fontSize: 12 }}>
                暂无捕获记录
              </div>
            )}
          </div>

          <div>
            <div style={{ fontSize: 12, fontWeight: 600, color: "var(--text-secondary)", marginBottom: 10 }}>
              最近捕获记录 ({gallery.length})
            </div>
            <div className="device-capture-gallery-grid">
              {gallery.map((item) => (
                <div
                  key={item.id}
                  className="device-capture-thumb-card"
                  onClick={() => setActiveItem(item)}
                >
                  <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
                    <Badge variant={item.type === "video" ? "warning" : "primary"}>
                      {item.type.toUpperCase()}
                    </Badge>
                    <span style={{ fontSize: 10, color: "var(--text-tertiary)" }}>{item.time}</span>
                  </div>
                  <strong style={{ fontSize: 11.5, color: "var(--text-primary)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                    {item.name}
                  </strong>
                  <span style={{ fontSize: 10.5, color: "var(--text-tertiary)" }}>{item.size || "--"}</span>
                </div>
              ))}
            </div>
          </div>
        </main>
      </div>
    </div>
  );
};

export default DeviceCaptureView;
