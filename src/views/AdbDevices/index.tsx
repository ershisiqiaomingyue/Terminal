import { ToolBackButton } from "@/components/ui/ToolBackButton";
import { useToolWindowValue, useToolDetachGuard } from "@/platform/toolWindowState";
import { SegmentedControl } from "@/components/ui/SegmentedControl";
import React, { useState, useEffect, useMemo, useRef } from "react";
import {
  RefreshCw,
  RotateCcw,
  Wifi,
  Copy,
  Terminal,
  BatteryMedium,
  HardDrive,
  Cpu,
  Package,
  Activity,
  ShieldCheck,
  Power,
  Play,
  Square,
  Trash2,
  Search,
  FileArchive,
  AlertTriangle,
} from "lucide-react";
import {
  Button,
  Input,
  Badge,
  PropertyGrid,
  type PropertyItem,
} from "@/components/ui";
import { ConfirmDialog } from "@/components/ConfirmDialog/ConfirmDialog";
import { useApplicationPageAction } from "@/application/useApplicationPageAction";
import { notify, publishAiTaskActivity, notifyDesktopRequired } from "@/platform/notifications";
import { invoke as runtimeInvoke } from "@/platform/runtime";
import "./adb-devices.css";

// ─── Tauri API 辅助 ───────────────────────────────────────────
const isTauriEnv = (): boolean =>
  typeof window !== "undefined" &&
  ("__TAURI_INTERNALS__" in window || "__TAURI__" in window);

async function safeInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (isTauriEnv()) {
    try {
      return await runtimeInvoke<T>(cmd, args);
    } catch (e) {
      console.warn(`[Tauri invoke ${cmd}] failed:`, e);
      throw e;
    }
  }
  throw new Error("NOT_TAURI");
}

async function safeOpenDialog(options: Record<string, unknown>): Promise<string | string[] | null> {
  try {
    const plugin: any = await new Function('return import("@tauri-apps/plugin-dialog")')();
    return await plugin.open(options);
  } catch {
    return null;
  }
}

async function safeSaveDialog(options: Record<string, unknown>): Promise<string | null> {
  try {
    const plugin: any = await new Function('return import("@tauri-apps/plugin-dialog")')();
    return await plugin.save(options);
  } catch {
    return null;
  }
}

// ─── 类型定义 (100% 对齐原版 AdbDevicesView.vue) ─────────────
export interface Device {
  serial: string;
  state: string;
  product?: string;
  model?: string;
  device?: string;
  transportId?: string;
}

export interface Info {
  androidVersion: string;
  sdk: string;
  manufacturer: string;
  model: string;
  buildFingerprint: string;
}

export interface Status {
  batteryLevel: string;
  batteryStatus: string;
  batteryTemperature: string;
  storage: string;
  abi: string;
  securityPatch: string;
  systemServicesReady: boolean;
  systemServicesMessage: string;
}

export interface PrivilegeStatus {
  isRoot: boolean;
  buildType: string;
  debuggable: boolean;
  secure: boolean;
  selinux: string;
  verifiedBootState: string;
  bootloaderLocked?: boolean;
  oemUnlockSupported?: boolean;
  slotSuffix: string;
  canRequestRoot: boolean;
}

export interface AppItem {
  packageName: string;
  sourcePath: string;
  kind: string; // "third-party" | "system"
}

export interface ProcessItem {
  pid: string;
  user: string;
  name: string;
}

export interface PerformanceCapability {
  name: string;
  available: boolean;
  detail: string;
}

export interface PerformanceMatrix {
  serial: string;
  capabilities: PerformanceCapability[];
}

export type TabType = "overview" | "apps" | "processes" | "diagnostics";

interface AdbDevicesViewProps {
  onBack?: () => void;
  onOpenTerminal?: (serial: string) => void;
  onOpenScrcpy?: (serial: string) => void;
}

export const AdbDevicesView: React.FC<AdbDevicesViewProps> = ({
  onBack,
  onOpenTerminal,
}) => {
  // ─── 状态定义 ──────────────────────────────────────────────
  const desktop = isTauriEnv();
  const [devices, setDevices] = useState<Device[]>([]);
  const [selectedSerial, setSelectedSerial] = useToolWindowValue("adb", "selectedSerial", "");
  const [info, setInfo] = useState<Info | null>(null);
  const [status, setStatus] = useState<Status | null>(null);
  const [privilege, setPrivilege] = useState<PrivilegeStatus | null>(null);
  const [packageCount, setPackageCount] = useState<number | null>(null);

  const initialTab =
    (typeof window !== "undefined"
      ? (new URLSearchParams(window.location.search).get("subtab") as TabType)
      : null) || "overview";
  const [tab, setTab] = useToolWindowValue<TabType>("adb", "tab", initialTab);
  const [apps, setApps] = useState<AppItem[]>([]);
  const [selectedApp, setSelectedApp] = useToolWindowValue<AppItem | null>("adb", "selectedApp", null);
  const [appSearch, setAppSearch] = useToolWindowValue("adb", "appSearch", "");

  const [processes, setProcesses] = useState<ProcessItem[]>([]);
  const [processSearch, setProcessSearch] = useToolWindowValue("adb", "processSearch", "");

  const [report, setReport] = useToolWindowValue("adb", "report", "battery");
  const [reportText, setReportText] = useToolWindowValue("adb", "reportText", "");
  const [graphicsPackage, setGraphicsPackage] = useToolWindowValue("adb", "graphicsPackage", "");
  const [performanceMatrix, setPerformanceMatrix] = useState<PerformanceMatrix | null>(null);

  // 加载状态
  const [loading, setLoading] = useState<boolean>(false);
  const [contentLoading, setContentLoading] = useState<boolean>(false);
  const [reportLoading, setReportLoading] = useState<boolean>(false);
  useToolDetachGuard("adb", loading || contentLoading || reportLoading);

  // 任务 ID
  const [bugreportTaskId, setBugreportTaskId] = useToolWindowValue("adb", "bugreportTaskId", "");
  const [crashDiagnosticsTaskId, setCrashDiagnosticsTaskId] = useToolWindowValue("adb", "crashDiagnosticsTaskId", "");
  const [healthCaptureTaskId, setHealthCaptureTaskId] = useToolWindowValue("adb", "healthCaptureTaskId", "");

  // 无线调试输入 (紧凑优雅)
  const [wirelessEndpoint, setWirelessEndpoint] = useState<string>("");
  const [pairingCode, setPairingCode] = useState<string>("");
  const [wirelessBusy, setWirelessBusy] = useState<string>("");

  // 弹窗与确认状态
  const [pendingAppAction, setPendingAppAction] = useState<string>("");
  const [pendingReboot, setPendingReboot] = useState<string>("");
  const [pendingRemount, setPendingRemount] = useState<boolean>(false);

  const showToast = (msg: string, type: "success" | "error" | "info" = "info") => {
    notify(msg, type);
  };

  // 当前选中的设备
  const currentDevice = useMemo(
    () => devices.find((d) => d.serial === selectedSerial) || devices[0] || null,
    [devices, selectedSerial]
  );

  // 应用过滤
  const filteredApps = useMemo(() => {
    const q = appSearch.trim().toLowerCase();
    if (!q) return apps;
    return apps.filter(
      (a) =>
        a.packageName.toLowerCase().includes(q) ||
        a.sourcePath.toLowerCase().includes(q) ||
        a.kind.toLowerCase().includes(q)
    );
  }, [apps, appSearch]);

  // 进程过滤
  const filteredProcesses = useMemo(() => {
    const q = processSearch.trim().toLowerCase();
    if (!q) return processes;
    return processes.filter(
      (p) =>
        p.pid.toLowerCase().includes(q) ||
        p.user.toLowerCase().includes(q) ||
        p.name.toLowerCase().includes(q)
    );
  }, [processes, processSearch]);

  const selectedSerialRef = useRef(selectedSerial);
  const detailsRequestRef = useRef(0);
  const devicesRequestRef = useRef(0);
  const contentRequestRef = useRef(0);
  const reportRequestRef = useRef(0);
  selectedSerialRef.current = selectedSerial;

  function clearDeviceData() {
    detailsRequestRef.current += 1;
    contentRequestRef.current += 1;
    reportRequestRef.current += 1;
    setContentLoading(false);
    setReportLoading(false);
    setInfo(null);
    setStatus(null);
    setPrivilege(null);
    setPackageCount(null);
    setApps([]);
    setSelectedApp(null);
    setProcesses([]);
    setReportText("");
    setPerformanceMatrix(null);
    setBugreportTaskId("");
    setCrashDiagnosticsTaskId("");
    setHealthCaptureTaskId("");
  }

  // 读取设备详情
  async function loadDetails(serialValue: string) {
    if (!serialValue || !desktop) return;
    const request = ++detailsRequestRef.current;
    try {
      const [infoRes, statusRes, packagesRes, privRes] = await Promise.allSettled([
          safeInvoke<Info>("get_adb_device_info", { serial: serialValue }),
          safeInvoke<Status>("get_adb_device_status", { serial: serialValue }),
          safeInvoke<string[]>("list_adb_packages", { serial: serialValue }),
          safeInvoke<PrivilegeStatus>("get_adb_privilege_status", { serial: serialValue }),
        ]);

        if (request !== detailsRequestRef.current || selectedSerialRef.current !== serialValue) return;
        setInfo(infoRes.status === "fulfilled" ? infoRes.value : null);
        setStatus(statusRes.status === "fulfilled" ? statusRes.value : null);
        setPackageCount(packagesRes.status === "fulfilled" ? packagesRes.value.length : null);
        setPrivilege(privRes.status === "fulfilled" ? privRes.value : null);

        const rejected = [infoRes, statusRes, packagesRes, privRes].find((item) => item.status === "rejected");
        if (rejected && rejected.status === "rejected") {
          showToast(String(rejected.reason), "error");
        }
    } catch (reason) {
      if (request !== detailsRequestRef.current) return;
      setInfo(null);
      setStatus(null);
      setPackageCount(null);
      setPrivilege(null);
      showToast(String(reason), "error");
    }
  }

  // 刷新设备列表
  async function refreshDevices(preferredSerial?: string) {
    if (!desktop) return;
    const request = ++devicesRequestRef.current;
    setLoading(true);
    try {
      const nextDevices = await safeInvoke<Device[]>("list_adb_devices");
      if (request !== devicesRequestRef.current) return;
      setDevices(nextDevices);
      const next =
        nextDevices.find((d) => d.serial === (preferredSerial || selectedSerialRef.current)) ||
        nextDevices.find((d) => d.state === "device") ||
        nextDevices[0];
      const newSerial = next?.serial || "";
      if (newSerial !== selectedSerialRef.current) clearDeviceData();
      selectedSerialRef.current = newSerial;
      setSelectedSerial(newSerial);
      if (newSerial) await loadDetails(newSerial);
    } catch (reason) {
      if (request !== devicesRequestRef.current) return;
      clearDeviceData();
      setDevices([]);
      selectedSerialRef.current = "";
      setSelectedSerial("");
      showToast(String(reason), "error");
      return false;
    } finally {
      if (request === devicesRequestRef.current) setLoading(false);
    }
  }

  useApplicationPageAction("open.tool.adb", (action) => {
    if (!desktop) { notifyDesktopRequired(action.actionId); return; }
    publishAiTaskActivity({ running: true, status: "running", title: "正在刷新 ADB 设备", detail: "正在扫描连接状态和设备信息。", actionId: action.actionId, progress: 15 });
    void refreshDevices(action.parameters.serial)
      .then((result) => publishAiTaskActivity({ running: false, status: result === false ? "error" : "done", title: result === false ? "ADB 设备刷新失败" : "ADB 设备已刷新", detail: result === false ? "未能读取设备列表。" : "已完成设备扫描。", actionId: action.actionId, progress: result === false ? null : 100 }))
      .catch((reason) => publishAiTaskActivity({ running: false, status: "error", title: "ADB 设备刷新失败", detail: String(reason), actionId: action.actionId }));
  });

  // 重启 ADB Server
  async function restartAdb() {
    if (!desktop) { notifyDesktopRequired(); return; }
    try {
      if (isTauriEnv()) {
        try {
          await safeInvoke("restart_adb");
        } catch (reason) {
          throw reason;
        }
      }
      showToast("ADB Server 重启完成，正在扫描设备...", "success");
      await refreshDevices();
    } catch (e) {
      showToast(String(e), "error");
    }
  }

  // 请求 Root 权限
  async function requestRoot() {
    if (!desktop) { notifyDesktopRequired(); return; }
    if (!selectedSerial) return;
    try {
      if (isTauriEnv()) {
        try {
          const nextPriv = await safeInvoke<PrivilegeStatus>("request_adb_root", {
            serial: selectedSerial,
          });
          setPrivilege(nextPriv);
          showToast(nextPriv.isRoot ? "adbd 已切换为 Root 身份。" : "已请求 adbd Root，但设备未返回 Root 身份。", "info");
          return;
        } catch (reason) {
          throw reason;
        }
      }
    } catch (e) {
      showToast(String(e), "error");
    }
  }

  // 重新挂载系统分区
  async function confirmRemount() {
    if (!desktop) { notifyDesktopRequired(); return; }
    if (!selectedSerial) return;
    setPendingRemount(false);
    try {
      if (isTauriEnv()) {
        try {
          const output = await safeInvoke<string>("remount_adb_system", {
            serial: selectedSerial,
          });
          showToast(output.trim() || "系统分区重新挂载命令已完成。", "success");
          return;
        } catch (reason) {
          throw reason;
        }
      }
    } catch (e) {
      showToast(String(e), "error");
    }
  }

  // 无线调试操作
  async function handleWirelessAction(action: "pair" | "connect" | "disconnect") {
    if (!desktop) { notifyDesktopRequired(); return; }
    if (!wirelessEndpoint.trim() || (action === "pair" && !pairingCode.trim())) {
      showToast("请填写完整的无线调试地址及配对码", "error");
      return;
    }
    setWirelessBusy(action);
    try {
      if (isTauriEnv()) {
        try {
          const output = await safeInvoke<string>(
            action === "pair"
              ? "pair_adb_wireless"
              : action === "connect"
              ? "connect_adb_wireless"
              : "disconnect_adb_wireless",
            action === "pair"
              ? {
                  request: {
                    endpoint: wirelessEndpoint,
                    pairingCode: pairingCode,
                  },
                }
              : { request: { endpoint: wirelessEndpoint } }
          );
          showToast(
            output ||
              (action === "pair"
                ? "无线调试配对成功！"
                : action === "connect"
                ? "无线设备已连接！"
                : "无线设备已断开。"),
            "success"
          );
          setPairingCode("");
          await refreshDevices();
          return;
        } catch (reason) {
          throw reason;
        }
      }
    } catch (e) {
      showToast(String(e), "error");
    } finally {
      setWirelessBusy("");
    }
  }

  // 加载应用包列表
  async function loadApps() {
    if (!selectedSerial || !desktop) return;
    const serial = selectedSerial;
    const request = ++contentRequestRef.current;
    setApps([]);
    setSelectedApp(null);
    setContentLoading(true);
    try {
      if (isTauriEnv()) {
        try {
          const appList = await safeInvoke<AppItem[]>("list_adb_applications", {
            serial: selectedSerial,
          });
          if (request === contentRequestRef.current && selectedSerialRef.current === serial) setApps(appList || []);
        } catch (reason) {
          if (request !== contentRequestRef.current || selectedSerialRef.current !== serial) return;
          setApps([]);
          showToast(String(reason), "error");
        }
      }
    } finally {
      if (request === contentRequestRef.current) setContentLoading(false);
    }
  }

  // 加载进程列表
  async function loadProcesses() {
    if (!selectedSerial || !desktop) return;
    const serial = selectedSerial;
    const request = ++contentRequestRef.current;
    setProcesses([]);
    setContentLoading(true);
    try {
      if (isTauriEnv()) {
        try {
          const procList = await safeInvoke<ProcessItem[]>("list_adb_process_details", {
            serial: selectedSerial,
          });
          if (request === contentRequestRef.current && selectedSerialRef.current === serial) setProcesses(procList || []);
        } catch (reason) {
          if (request !== contentRequestRef.current || selectedSerialRef.current !== serial) return;
          setProcesses([]);
          showToast(String(reason), "error");
        }
      }
    } finally {
      if (request === contentRequestRef.current) setContentLoading(false);
    }
  }

  // 加载诊断信息
  async function loadReport(targetReport?: string) {
    const rep = targetReport || report;
    if (!selectedSerial || reportLoading || !desktop) return;
    const serial = selectedSerial;
    const request = ++reportRequestRef.current;
    setReportLoading(true);
    setReportText("");
    setPerformanceMatrix(null);
    try {
      if (isTauriEnv()) {
        try {
          if (rep === "performance-support") {
            const matrix = await safeInvoke<PerformanceMatrix>(
              "get_android_performance_support_matrix",
              { serial: selectedSerial }
            );
            if (request === reportRequestRef.current && selectedSerialRef.current === serial) setPerformanceMatrix(matrix);
          } else {
            const text =
              rep === "graphics"
                ? await safeInvoke<string>("get_adb_graphics_diagnostic", {
                    request: {
                      serial: selectedSerial,
                      packageName: graphicsPackage,
                    },
                  })
                : await safeInvoke<string>("get_adb_diagnostic", {
                    serial: selectedSerial,
                    report: rep,
                  });
            if (request === reportRequestRef.current && selectedSerialRef.current === serial) setReportText(text || "");
          }
        } catch (reason) {
          if (request !== reportRequestRef.current || selectedSerialRef.current !== serial) return;
          setReportText("");
          showToast(String(reason), "error");
        }
      }
    } finally {
      if (request === reportRequestRef.current) {
        setReportLoading(false);
      }
    }
  }

  // 采集 Bugreport
  async function collectBugreport() {
    if (!desktop) { notifyDesktopRequired(); return; }
    if (!selectedSerial) return;
    try {
      const destination = await safeSaveDialog({
        title: "保存 Bugreport 诊断包",
        defaultPath: `bugreport-${selectedSerial.replace(/[^a-zA-Z0-9._-]/g, "_")}.zip`,
        filters: [{ name: "Bugreport ZIP", extensions: ["zip"] }],
      });
      if (!destination) return;
      if (isTauriEnv()) {
        try {
          const taskId = await safeInvoke<string>("start_adb_bugreport", {
            request: { serial: selectedSerial, destination },
          });
          setBugreportTaskId(taskId);
          showToast("Bugreport 已开始采集，可在任务记录中查看进度。", "info");
          return;
        } catch (reason) {
          throw reason;
        }
      }
    } catch (e) {
      showToast(String(e), "error");
    }
  }

  // 采集 ANR 与崩溃
  async function collectCrashDiagnostics() {
    if (!desktop) { notifyDesktopRequired(); return; }
    if (!selectedSerial) return;
    try {
      const destination = await safeOpenDialog({
        title: "选择 ANR 与崩溃诊断包目录",
        directory: true,
        multiple: false,
      });
      if (!destination || Array.isArray(destination)) return;
      if (isTauriEnv()) {
        try {
          const taskId = await safeInvoke<string>("start_crash_diagnostics", {
            request: {
              serial: selectedSerial,
              destination: `${destination}\\android-diagnostics-${Date.now()}`,
            },
          });
          setCrashDiagnosticsTaskId(taskId);
          showToast("ANR 与崩溃诊断包正在采集，可在任务记录中查看进度。", "info");
          return;
        } catch (reason) {
          throw reason;
        }
      }
    } catch (e) {
      showToast(String(e), "error");
    }
  }

  // 采集设备健康快照
  async function collectDeviceHealth() {
    if (!desktop) { notifyDesktopRequired(); return; }
    if (!selectedSerial) return;
    try {
      const destination = await safeOpenDialog({
        title: "选择设备健康快照保存目录",
        directory: true,
        multiple: false,
      });
      if (!destination || Array.isArray(destination)) return;
      if (isTauriEnv()) {
        try {
          const taskId = await safeInvoke<string>("start_device_health_capture", {
            request: { serial: selectedSerial, destination },
          });
          setHealthCaptureTaskId(taskId);
          showToast("设备健康快照正在采集，可在任务记录中查看进度和产物。", "info");
          return;
        } catch (reason) {
          throw reason;
        }
      }
    } catch (e) {
      showToast(String(e), "error");
    }
  }

  // 切换选择设备
  function handleSelectDevice(value: string) {
    if (selectedSerial === value) return;
    clearDeviceData();
    selectedSerialRef.current = value;
    setSelectedSerial(value);
    setReportText("");
    setPerformanceMatrix(null);
    void loadDetails(value);
  }

  // 复制序列号
  async function handleCopySerial() {
    if (currentDevice?.serial) {
      await navigator.clipboard.writeText(currentDevice.serial);
      showToast(`已复制设备序列号: ${currentDevice.serial}`, "success");
    }
  }

  // 应用包操作
  const appActionLabels: Record<string, string> = {
    launch: "启动",
    "force-stop": "强制停止",
    "clear-data": "清除数据",
    uninstall: "卸载",
  };

  async function executeAppAction(action: string) {
    if (!desktop) { notifyDesktopRequired(); return; }
    if (!selectedApp || !selectedSerial) return;
    try {
      if (isTauriEnv()) {
        try {
          const result = await safeInvoke<string>("run_adb_package_action", {
            serial: selectedSerial,
            packageName: selectedApp.packageName,
            action,
          });
          showToast(`${appActionLabels[action]}完成${result.trim() ? `：${result.trim()}` : ""}`, "success");
          if (action === "uninstall") {
            setApps((prev) => prev.filter((a) => a.packageName !== selectedApp.packageName));
            setSelectedApp(null);
          }
          return;
        } catch (reason) {
          throw reason;
        }
      }
    } catch (e) {
      showToast(String(e), "error");
    }
  }

  function handleAppActionClick(action: string) {
    if (!selectedApp) return;
    if (["clear-data", "uninstall", "force-stop", "disable"].includes(action)) {
      setPendingAppAction(action);
      return;
    }
    void executeAppAction(action);
  }

  // 设备重启操作
  const rebootLabels: Record<string, string> = {
    normal: "重启设备",
    recovery: "进入 Recovery",
    bootloader: "进入 Bootloader",
  };

  async function executeReboot(mode: string) {
    if (!desktop) { notifyDesktopRequired(); return; }
    if (!selectedSerial || !mode) return;
    setPendingReboot("");
    try {
      if (isTauriEnv()) {
        try {
          await safeInvoke("restart_adb_device", { serial: selectedSerial, mode });
          showToast(`${rebootLabels[mode]}命令已发送。`, "success");
          return;
        } catch (reason) {
          throw reason;
        }
      }
    } catch (e) {
      showToast(String(e), "error");
    }
  }

  // 遥测属性网格配置数据 (13项)
  const telemetryItems = useMemo<PropertyItem[]>(() => {
    return [
      {
        label: "电池状态",
        icon: <BatteryMedium size={14} />,
        value: `${status?.batteryLevel || "未知"} · ${status?.batteryStatus || "未知"}`,
      },
      {
        label: "数据分区",
        icon: <HardDrive size={14} />,
        value: status?.storage || "未知",
      },
      {
        label: "ABI 架构",
        icon: <Cpu size={14} />,
        value: status?.abi || "未知",
      },
      {
        label: "已安装应用包",
        icon: <Package size={14} />,
        value: packageCount === null ? "未知" : `${packageCount} 个`,
      },
      {
        label: "电池温度",
        icon: <BatteryMedium size={14} />,
        value: status?.batteryTemperature || "未知",
      },
      {
        label: "安全补丁版本",
        icon: <Activity size={14} />,
        value: status?.securityPatch || "未知",
      },
      {
        label: "构建类型",
        icon: <ShieldCheck size={14} />,
        value: privilege?.buildType || "未知",
      },
      {
        label: "ADB Root 状态",
        icon: <ShieldCheck size={14} />,
        value: privilege
          ? privilege.isRoot
            ? "已获取"
            : privilege.canRequestRoot
            ? "可申请"
            : "不可用"
          : "未知",
        valueType: privilege?.isRoot ? "green" : undefined,
      },
      {
        label: "SELinux 状态",
        icon: <ShieldCheck size={14} />,
        value: privilege?.selinux || "未知",
      },
      {
        label: "Verified Boot",
        icon: <ShieldCheck size={14} />,
        value: privilege?.verifiedBootState || "未知",
      },
      {
        label: "Bootloader 锁定",
        icon: <ShieldCheck size={14} />,
        value:
          privilege?.bootloaderLocked === true
            ? "已锁定"
            : privilege?.bootloaderLocked === false
            ? "已解锁"
            : "未知",
        valueType: privilege?.bootloaderLocked === false ? "orange" : undefined,
      },
      {
        label: "OEM 解锁支持",
        icon: <ShieldCheck size={14} />,
        value:
          privilege?.oemUnlockSupported === true
            ? "支持"
            : privilege?.oemUnlockSupported === false
            ? "不支持"
            : "未知",
      },
      {
        label: "当前启动槽位",
        icon: <ShieldCheck size={14} />,
        value: privilege?.slotSuffix || "未知",
      },
    ];
  }, [status, packageCount, privilege]);

  // 监听 Tab 切换，按需懒加载
  useEffect(() => {
    if (tab === "apps") void loadApps();
    if (tab === "processes") void loadProcesses();
    if (tab === "diagnostics" && !reportText && !performanceMatrix) void loadReport();
  }, [tab, selectedSerial]);

  // 当有应用列表且未选中时，默认选中第一个应用展示右侧操作区
  useEffect(() => {
    if (apps.length > 0 && !selectedApp) {
      setSelectedApp(apps[0]);
    }
  }, [apps, selectedApp]);

  // 挂载自动刷新
  useEffect(() => {
    void refreshDevices();
    const heartbeat = setInterval(() => {
      void refreshDevices();
    }, 12000);
    return () => {
      clearInterval(heartbeat);
      detailsRequestRef.current += 1;
      devicesRequestRef.current += 1;
      contentRequestRef.current += 1;
      reportRequestRef.current += 1;
    };
  }, []);

  return (
    <div className="adb-dev-container">
      {/* 顶部标题栏 */}
      <header className="adb-dev-top-header">
        <div className="adb-dev-header-left">
          <ToolBackButton onBack={onBack} />
          <div className="adb-dev-title-wrap">
            <h1 className="adb-dev-title">设备工作台</h1>
            <span className="adb-dev-subtitle">通过 Android Debug Bridge 管理设备、应用、进程和系统诊断</span>
          </div>
        </div>
        <div className="adb-dev-header-right">
          <Button
            variant="subtle"
            size="sm"
            icon={<RefreshCw size={14} className={loading ? "spin" : ""} />}
            onClick={() => void refreshDevices()}
            disabled={loading}
          >
            刷新设备
          </Button>
          <Button
            variant="secondary"
            size="sm"
            icon={<RotateCcw size={14} />}
            onClick={() => void restartAdb()}
          >
            重启 ADB
          </Button>
        </div>
      </header>

      {/* 无线调试工具条：左侧标题与说明，右侧端口/配对码/操作按钮整体靠右 */}
      <section className="adb-dev-wireless-bar">
        <div className="adb-dev-wireless-label">
          <Wifi size={15} />
          <strong>无线调试</strong>
          <small>配对端口与连接端口通常不同。</small>
        </div>
        <div className="adb-dev-wireless-right-group">
          <div className="adb-dev-endpoint-input-wrap">
            <Input
              value={wirelessEndpoint}
              onChange={(e) => setWirelessEndpoint(e.target.value)}
              disabled={Boolean(wirelessBusy)}
              placeholder="地址:端口，例如 192.168.1.20:5555"
              size="sm"
              onKeyDown={(e) => {
                if (e.key === "Enter") void handleWirelessAction("connect");
              }}
            />
          </div>
          <div className="adb-dev-pairing-input-wrap">
            <Input
              value={pairingCode}
              onChange={(e) => setPairingCode(e.target.value)}
              disabled={Boolean(wirelessBusy)}
              placeholder="六位配对码"
              size="sm"
              maxLength={6}
              onKeyDown={(e) => {
                if (e.key === "Enter") void handleWirelessAction("pair");
              }}
            />
          </div>
          <div className="adb-dev-wireless-actions">
            <Button
              variant="secondary"
              size="sm"
              disabled={Boolean(wirelessBusy) || !wirelessEndpoint || !pairingCode}
              onClick={() => void handleWirelessAction("pair")}
            >
              {wirelessBusy === "pair" ? "配对中" : "配对"}
            </Button>
            <Button
              variant="primary"
              size="sm"
              disabled={Boolean(wirelessBusy) || !wirelessEndpoint}
              onClick={() => void handleWirelessAction("connect")}
            >
              {wirelessBusy === "connect" ? "连接中" : "连接"}
            </Button>
            <Button
              variant="secondary"
              size="sm"
              disabled={Boolean(wirelessBusy) || !wirelessEndpoint}
              onClick={() => void handleWirelessAction("disconnect")}
            >
              {wirelessBusy === "disconnect" ? "断开中" : "断开"}
            </Button>
          </div>
        </div>
      </section>

      {/* 主分栏工作区：左侧设备侧边栏 + 右侧详情大卡片 */}
      <div className="adb-dev-body-split">
        {/* 左侧：已连接设备列表 */}
        <aside className="adb-dev-aside">
          <div className="adb-dev-aside-header">
            <span>已连接设备</span>
            <span className="adb-dev-badge-count">{devices.length}</span>
          </div>

          <div className="adb-dev-device-list custom-scrollbar">
            {devices.map((item) => {
              const isSelected = selectedSerial === item.serial;
              const isOnline = item.state === "device";
              return (
                <div
                  key={item.serial}
                  className={`adb-dev-item-row ${isSelected ? "selected" : ""}`}
                  onClick={() => handleSelectDevice(item.serial)}
                >
                  <div className="adb-dev-item-lamp-col">
                    <span className={`adb-dev-lamp ${isOnline ? "ok" : "warn"}`} />
                  </div>
                  <div className="adb-dev-item-info-col">
                    <strong className="adb-dev-item-model">
                      {item.model || item.serial}
                    </strong>
                    <span className="adb-dev-item-serial">{item.serial}</span>
                  </div>
                  <div className="adb-dev-item-status-col">
                    <Badge
                      variant={isOnline ? "success" : item.state === "unauthorized" ? "danger" : "warning"}
                    >
                      {isOnline ? "在线" : item.state === "unauthorized" ? "未授权" : item.state}
                    </Badge>
                  </div>
                </div>
              );
            })}
          </div>
        </aside>

        {/* 右侧：当前选中设备的详情工作卡片 */}
        <main className="adb-dev-article">
          {currentDevice ? (
            <>
              {/* 设备详情卡片头部 */}
              <div className="adb-dev-card-header">
                <div className="adb-dev-card-title-col">
                  <h2>{info?.model || currentDevice.model || "Android 设备"}</h2>
                  <p className="adb-dev-card-meta">
                    {info?.manufacturer || currentDevice.product || "设备信息"}
                    {info && (
                      <span>
                        {" · "}Android {info.androidVersion} · SDK {info.sdk}
                      </span>
                    )}
                  </p>
                </div>
                <div className="adb-dev-card-actions">
                  <Button
                    variant="secondary"
                    size="sm"
                    icon={<Copy size={13} />}
                    onClick={() => void handleCopySerial()}
                  >
                    复制序列号
                  </Button>
                  {onOpenTerminal && (
                    <Button
                      variant="primary"
                      size="sm"
                      icon={<Terminal size={13} />}
                      onClick={() => onOpenTerminal(currentDevice.serial)}
                    >
                      去终端
                    </Button>
                  )}
                </div>
              </div>

              {/* 四大 Tab 导航切换 (使用 .segmented-glass 标准胶囊分段器) */}
              <div className="adb-dev-tabs-container">
                <SegmentedControl value={tab} onChange={setTab} label="设备视图" className="adb-dev-seg-group" options={[
                  { value: "overview", label: "设备总览" }, { value: "apps", label: "应用包" },
                  { value: "processes", label: "运行进程" }, { value: "diagnostics", label: "系统诊断" },
                ]} />
              </div>

              {/* 系统服务警示条 (若系统服务尚未完全就绪) */}
              {status && !status.systemServicesReady && (
                <div className="adb-dev-service-warning">
                  <AlertTriangle size={15} />
                  <span>
                    {status.systemServicesMessage} ADB 连接仍可用于 Logcat、getprop 等基础操作；包管理和 dumpsys 诊断将在系统服务恢复后可用。
                  </span>
                </div>
              )}

              {/* ─── TAB 1: 设备总览 ───────────────────────────── */}
              {tab === "overview" && (
                <div className="adb-dev-tab-pane adb-dev-pane-overview custom-scrollbar">
                  {/* 13 项标准遥测属性网格 */}
                  <div className="adb-dev-telemetry-box">
                    <PropertyGrid items={telemetryItems} columns={2} />
                  </div>

                  {/* 设备操作矩阵卡片 */}
                  <div className="adb-dev-actions-matrix">
                    <div className="adb-dev-actions-label">设备快速控制</div>
                    <div className="adb-dev-actions-buttons">
                      <Button
                        variant="danger"
                        size="sm"
                        icon={<Power size={13} />}
                        onClick={() => setPendingReboot("normal")}
                      >
                        重启设备
                      </Button>
                      <Button
                        variant="danger"
                        size="sm"
                        onClick={() => setPendingReboot("recovery")}
                      >
                        进入 Recovery
                      </Button>
                      <Button
                        variant="danger"
                        size="sm"
                        onClick={() => setPendingReboot("bootloader")}
                      >
                        进入 Bootloader
                      </Button>
                      {privilege?.canRequestRoot && !privilege.isRoot && (
                        <Button
                          variant="secondary"
                          size="sm"
                          onClick={() => void requestRoot()}
                        >
                          请求 Root
                        </Button>
                      )}
                      {privilege?.isRoot && (
                        <Button
                          variant="secondary"
                          size="sm"
                          onClick={() => setPendingRemount(true)}
                        >
                          重新挂载 (Remount)
                        </Button>
                      )}
                    </div>
                  </div>

                  {/* 指纹与序列号底座 */}
                  <div className="adb-dev-fingerprint-panel">
                    <div className="adb-dev-code-block">
                      <span className="adb-dev-code-label">Build Fingerprint</span>
                      <code>{info?.buildFingerprint || "暂不可用"}</code>
                    </div>
                    <div className="adb-dev-code-block">
                      <span className="adb-dev-code-label">
                        设备序列号 · Transport {currentDevice.transportId || "未知"}
                      </span>
                      <code>{currentDevice.serial}</code>
                    </div>
                  </div>
                </div>
              )}

              {/* ─── TAB 2: 应用包 ─────────────────────────────── */}
              {tab === "apps" && (
                <div className="adb-dev-tab-pane adb-dev-pane-apps">
                  {/* 工具条 */}
                  <div className="adb-dev-pane-toolbar">
                    <div className="adb-dev-search-box">
                      <Input
                        placeholder="搜索包名、APK 路径或类型..."
                        value={appSearch}
                        onChange={(e) => setAppSearch(e.target.value)}
                        prefixIcon={<Search size={14} />}
                        size="sm"
                      />
                    </div>
                    <div className="adb-dev-toolbar-right">
                      <span className="adb-dev-stats-text">
                        {contentLoading
                          ? "正在读取应用包..."
                          : `${filteredApps.length} / ${apps.length} 个应用`}
                      </span>
                      <Button
                        variant="subtle"
                        size="sm"
                        icon={<RefreshCw size={13} className={contentLoading ? "spin" : ""} />}
                        onClick={() => void loadApps()}
                      />
                    </div>
                  </div>

                  {/* 左右分栏：左应用列表 + 右应用快捷控制 */}
                  <div className="adb-dev-apps-split">
                    <section className="adb-dev-apps-list-box custom-scrollbar">
                      {filteredApps.map((app) => {
                        const isSelected = selectedApp?.packageName === app.packageName;
                        return (
                          <div
                            key={app.packageName}
                            className={`adb-dev-app-row ${isSelected ? "selected" : ""}`}
                            onClick={() => setSelectedApp(app)}
                          >
                            <Package size={15} className="adb-dev-app-icon" />
                            <div className="adb-dev-app-meta">
                              <strong className="adb-dev-app-name">{app.packageName}</strong>
                              <span className="adb-dev-app-path">{app.sourcePath}</span>
                            </div>
                            <span className="adb-dev-app-tag">{app.kind}</span>
                          </div>
                        );
                      })}
                      {!contentLoading && !filteredApps.length && (
                        <div className="adb-dev-list-blank">没有匹配的应用包</div>
                      )}
                    </section>

                    {/* 右侧：当前选中应用详情 */}
                    <aside className="adb-dev-app-detail-card">
                      {selectedApp ? (
                        <>
                          <small className="adb-dev-card-hint">当前选中应用</small>
                          <h3 className="adb-dev-selected-app-name">{selectedApp.packageName}</h3>
                          <code className="adb-dev-selected-app-path">{selectedApp.sourcePath}</code>

                          <div className="adb-dev-app-actions-row">
                            <Button
                              variant="secondary"
                              size="sm"
                              icon={<Play size={13} />}
                              onClick={() => handleAppActionClick("launch")}
                            >
                              启动
                            </Button>
                            <Button
                              variant="danger"
                              size="sm"
                              icon={<Square size={13} />}
                              onClick={() => handleAppActionClick("force-stop")}
                            >
                              强制停止
                            </Button>
                            <Button
                              variant="danger"
                              size="sm"
                              onClick={() => handleAppActionClick("clear-data")}
                            >
                              清除数据
                            </Button>
                            <Button
                              variant="danger"
                              size="sm"
                              icon={<Trash2 size={13} />}
                              onClick={() => handleAppActionClick("uninstall")}
                            >
                              卸载
                            </Button>
                          </div>
                        </>
                      ) : (
                        <div className="adb-dev-list-blank">请在左侧选择一个应用包以执行操作</div>
                      )}
                    </aside>
                  </div>
                </div>
              )}

              {/* ─── TAB 3: 运行进程 ───────────────────────────── */}
              {tab === "processes" && (
                <div className="adb-dev-tab-pane adb-dev-pane-processes">
                  {/* 工具条 */}
                  <div className="adb-dev-pane-toolbar">
                    <div className="adb-dev-search-box">
                      <Input
                        placeholder="搜索 PID、运行用户或进程名称..."
                        value={processSearch}
                        onChange={(e) => setProcessSearch(e.target.value)}
                        prefixIcon={<Search size={14} />}
                        size="sm"
                      />
                    </div>
                    <div className="adb-dev-toolbar-right">
                      <span className="adb-dev-stats-text">
                        {contentLoading ? "正在读取进程..." : `${filteredProcesses.length} 个进程`}
                      </span>
                      <Button
                        variant="subtle"
                        size="sm"
                        icon={<RefreshCw size={13} className={contentLoading ? "spin" : ""} />}
                        onClick={() => void loadProcesses()}
                      />
                    </div>
                  </div>

                  {/* 进程列表表格 (Grid 虚拟表) */}
                  <div className="adb-dev-proc-table custom-scrollbar">
                    <div className="adb-dev-proc-tr adb-dev-proc-th">
                      <span className="adb-dev-col-pid">PID</span>
                      <span className="adb-dev-col-user">用户</span>
                      <span className="adb-dev-col-name">进程名称</span>
                    </div>
                    {filteredProcesses.map((p) => (
                      <div key={`${p.pid}-${p.name}`} className="adb-dev-proc-tr">
                        <code className="adb-dev-col-pid">{p.pid}</code>
                        <span className="adb-dev-col-user">{p.user}</span>
                        <strong className="adb-dev-col-name">{p.name}</strong>
                      </div>
                    ))}
                    {!contentLoading && !filteredProcesses.length && (
                      <div className="adb-dev-list-blank">未找到相关进程</div>
                    )}
                  </div>
                </div>
              )}

              {/* ─── TAB 4: 系统诊断 ───────────────────────────── */}
              {tab === "diagnostics" && (
                <div className="adb-dev-tab-pane adb-dev-pane-diagnostics">
                  {/* 诊断分类按钮分段栏 + 诊断采集动作按钮 */}
                  <div className="adb-dev-diagnostics-top">
                    <div className="adb-dev-diag-chips">
                      {[
                        { id: "battery", name: "电池" },
                        { id: "battery-stats", name: "耗电" },
                        { id: "thermal", name: "温度" },
                        { id: "memory", name: "内存" },
                        { id: "cpu", name: "CPU" },
                        { id: "activities", name: "Activity" },
                        { id: "network", name: "网络" },
                        { id: "graphics", name: "渲染" },
                        { id: "surfaceflinger", name: "GPU / 合成" },
                        { id: "performance-support", name: "性能支持" },
                      ].map((item) => (
                        <button
                          key={item.id}
                          className={`adb-dev-diag-chip ${report === item.id ? "active" : ""}`}
                          onClick={() => {
                            setReport(item.id);
                            void loadReport(item.id);
                          }}
                        >
                          {item.name}
                        </button>
                      ))}
                    </div>

                    <div className="adb-dev-diag-actions">
                      <Button
                        variant="secondary"
                        size="sm"
                        icon={<RefreshCw size={13} className={reportLoading ? "spin" : ""} />}
                        onClick={() => void loadReport()}
                      >
                        刷新
                      </Button>
                      <Button
                        variant="secondary"
                        size="sm"
                        icon={<HardDrive size={13} />}
                        disabled={Boolean(healthCaptureTaskId)}
                        onClick={() => void collectDeviceHealth()}
                      >
                        {healthCaptureTaskId ? "正在保存快照..." : "保存健康快照"}
                      </Button>
                      <Button
                        variant="secondary"
                        size="sm"
                        icon={<FileArchive size={13} />}
                        disabled={Boolean(bugreportTaskId)}
                        onClick={() => void collectBugreport()}
                      >
                        {bugreportTaskId ? "正在采集..." : "采集 Bugreport"}
                      </Button>
                      <Button
                        variant="secondary"
                        size="sm"
                        icon={<FileArchive size={13} />}
                        disabled={Boolean(crashDiagnosticsTaskId)}
                        onClick={() => void collectCrashDiagnostics()}
                      >
                        {crashDiagnosticsTaskId ? "正在采集证据..." : "采集 ANR / 崩溃"}
                      </Button>
                    </div>
                  </div>

                  {/* 针对渲染 diagnostics 的专属包名输入框 */}
                  {report === "graphics" && (
                    <div className="adb-dev-graphics-filter-row">
                      <span className="adb-dev-graphics-filter-label">应用包名（留空查看全设备）：</span>
                      <Input
                        value={graphicsPackage}
                        onChange={(e) => setGraphicsPackage(e.target.value)}
                        placeholder="例如 com.example.app"
                        size="sm"
                        className="adb-dev-graphics-filter-input"
                        onKeyDown={(e) => {
                          if (e.key === "Enter") void loadReport("graphics");
                        }}
                      />
                      <Button variant="secondary" size="sm" onClick={() => void loadReport("graphics")}>
                        查询
                      </Button>
                    </div>
                  )}

                  {/* 针对性能支持专属的能力矩阵 */}
                  {report === "performance-support" ? (
                    <div className="adb-dev-perf-matrix-box">
                      <div className="adb-dev-perf-matrix-header">
                        <div>
                          <strong>设备性能能力</strong>
                          <small>仅检测系统组件可用性，不会对设备做任何修改</small>
                        </div>
                        {performanceMatrix && (
                          <span className="adb-dev-perf-serial">{performanceMatrix.serial}</span>
                        )}
                      </div>

                      {reportLoading ? (
                        <div className="adb-dev-list-blank">
                          正在检测 Perfetto、Simpleperf、Atrace 与渲染诊断能力...
                        </div>
                      ) : performanceMatrix ? (
                        <div className="adb-dev-perf-list custom-scrollbar">
                          {performanceMatrix.capabilities.map((item) => (
                            <div
                              key={item.name}
                              className={`adb-dev-perf-row ${!item.available ? "unavailable" : ""}`}
                            >
                              <span className={`adb-dev-perf-badge ${item.available ? "ok" : "fail"}`}>
                                {item.available ? "可用" : "不可用"}
                              </span>
                              <div className="adb-dev-perf-info">
                                <strong>{item.name}</strong>
                                <p>{item.detail.trim() || "设备没有返回可用的能力证据。"}</p>
                              </div>
                            </div>
                          ))}
                        </div>
                      ) : (
                        <div className="adb-dev-list-blank">尚未检测设备性能能力</div>
                      )}
                    </div>
                  ) : (
                    /* 标准终端诊断输出：原版无边框独立命令指示行 + One Dark Pro 灰底圆角 Pre 容器 */
                    <div className="adb-dev-diag-terminal-wrap">
                      <p className="adb-dev-diag-command">
                        <span className="adb-dev-diag-cmd-prompt">&gt;_</span>
                        <span className="adb-dev-diag-cmd-text">
                          adb shell{" "}
                          {report === "network"
                            ? "dumpsys netstats"
                            : `dumpsys ${
                                report === "battery-stats"
                                  ? "batterystats --charged"
                                  : report === "thermal"
                                  ? "thermalservice"
                                  : report === "memory"
                                  ? "meminfo"
                                  : report === "cpu"
                                  ? "cpuinfo"
                                  : report === "activities"
                                  ? "activity activities"
                                  : report === "graphics"
                                  ? `gfxinfo${graphicsPackage ? ` ${graphicsPackage}` : ""}`
                                  : report === "surfaceflinger"
                                  ? "SurfaceFlinger"
                                  : "battery"
                              }`}
                        </span>
                      </p>
                      <pre className="adb-dev-diag-pre custom-scrollbar">
                        {reportLoading ? "正在读取设备诊断输出..." : reportText || "暂无诊断输出数据"}
                      </pre>
                    </div>
                  )}
                </div>
              )}

          </> ) : (
            <div className="adb-dev-empty-state">
              <p>请选择一个左侧设备以查看详情</p>
            </div>
          )}
        </main>
      </div>

      {/* ─── 确认弹窗集 ───────────────────────────────────────────── */}
      {/* 应用操作二次确认弹窗 */}
      <ConfirmDialog
        open={Boolean(pendingAppAction)}
        title={`${appActionLabels[pendingAppAction] || "确认"}应用包`}
        message={`将对 ${selectedApp?.packageName || "当前应用"} 执行${
          appActionLabels[pendingAppAction] || "此"
        }操作。${
          pendingAppAction === "clear-data"
            ? "应用数据将被彻底清除且无法撤销。"
            : pendingAppAction === "uninstall"
            ? "该应用将从设备中彻底卸载。"
            : pendingAppAction === "force-stop"
            ? "应用进程将立即停止，未保存的数据可能丢失。"
            : pendingAppAction === "disable"
            ? "应用将无法启动，停用系统应用可能影响设备功能。"
            : ""
        }`}
        confirmLabel="继续执行"
        onCancel={() => setPendingAppAction("")}
        onConfirm={() => {
          const act = pendingAppAction;
          setPendingAppAction("");
          void executeAppAction(act);
        }}
      />

      {/* 设备重启二次确认弹窗 */}
      <ConfirmDialog
        open={Boolean(pendingReboot)}
        title={rebootLabels[pendingReboot] || "确认设备操作"}
        message={`将向当前设备发送【${
          rebootLabels[pendingReboot] || "操作"
        }】命令，设备将立即重启，未保存的数据可能会丢失。`}
        confirmLabel="确认重启"
        onCancel={() => setPendingReboot("")}
        onConfirm={() => {
          const mode = pendingReboot;
          setPendingReboot("");
          void executeReboot(mode);
        }}
      />

      {/* 重新挂载系统分区二次确认弹窗 */}
      <ConfirmDialog
        open={pendingRemount}
        title="重新挂载系统分区"
        message="将执行 adb remount，系统分区将被重新挂载为可读写模式。仅在明确需要修改系统文件时继续。"
        confirmLabel="重新挂载"
        onCancel={() => setPendingRemount(false)}
        onConfirm={() => void confirmRemount()}
      />
    </div>
  );
};

export default AdbDevicesView;
