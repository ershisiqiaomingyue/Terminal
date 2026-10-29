import { ToolBackButton } from "@/components/ui/ToolBackButton";
import { readToolWindowState, useToolWindowValue, useToolDetachGuard } from "@/platform/toolWindowState";
import { SegmentedControl } from "@/components/ui/SegmentedControl";
import React, { useEffect, useMemo, useRef, useState } from "react";
import {
  Package,
  Upload,
  RefreshCw,
  Search,
  Terminal,
  Play,
  Square,
  Download,
  Trash2,
  Power,
  ShieldCheck,
  Smartphone,
  HardDrive,
  Cpu,
  Layers,
  Activity,
  FileCode,
  Copy,
} from "lucide-react";
import {
  Button,
  Input,
  Badge,
  Select,
  PropertyGrid,
  PropertyItem,
  Divider,
} from "@/components/ui";
import { ConfirmDialog } from "@/components/ConfirmDialog/ConfirmDialog";
import { useConfirmAction } from "@/components/ConfirmDialog/useConfirmAction";
import { useApplicationPageAction } from "@/application/useApplicationPageAction";
import { notify, publishAiTaskActivity, notifyDesktopRequired } from "@/platform/notifications";
import {
  deviceOptionLabel,
  formatInvokeError,
  invoke,
  isTauri,
  listAdbDevices,
  openDirectory,
  openFile,
  type AdbDevice,
} from "@/platform/runtime";
import "./package-manager.css";

interface PackageManagerViewProps {
  onBack?: () => void;
  onOpenTerminal?: (serial: string) => void;
}

interface AppItem {
  packageName: string;
  sourcePath: string;
  kind: "第三方" | "系统内置";
  versionName: string;
  versionCode: string;
  uid: string;
  enabled: boolean | null;
  permissions: { name: string; granted: boolean | null }[];
}

interface ListedApplication {
  packageName: string;
  sourcePath: string;
  kind: string;
}

interface PackageDetails {
  packageName: string;
  versionName: string;
  versionCode: string;
  uid: string;
  enabled: string;
  sourcePaths: string[];
  requestedPermissions: string[];
  permissions?: { name: string; granted: boolean | null }[];
  enabledState?: boolean | null;
}

interface DeviceOption {
  serial: string;
  model: string;
  state: string;
  label: string;
}

type PackageAction = "launch" | "force-stop" | "clear-data" | "uninstall" | "enable" | "disable";

function toDeviceOption(device: AdbDevice): DeviceOption {
  return {
    serial: device.serial,
    model: device.model || device.product || device.serial,
    state: device.state,
    label: deviceOptionLabel(device),
  };
}

function toAppItem(app: ListedApplication): AppItem {
  const thirdParty = app.kind === "用户" || app.sourcePath.startsWith("/data/");
  return {
    packageName: app.packageName,
    sourcePath: app.sourcePath,
    kind: thirdParty ? "第三方" : "系统内置",
    versionName: "-",
    versionCode: "-",
    uid: "-",
    enabled: null,
    permissions: [],
  };
}

export const PackageManagerView: React.FC<PackageManagerViewProps> = ({
  onBack,
  onOpenTerminal,
}) => {
  const tauri = isTauri();
  const [devices, setDevices] = useState<DeviceOption[]>(() => []);
  const [selectedSerial, setSelectedSerial] = useToolWindowValue("apps", "selectedSerial", "");
  const [isRefreshing, setIsRefreshing] = useState(false);

  const [apps, setApps] = useState<AppItem[]>(() => []);
  const [selectedApp, setSelectedApp] = useToolWindowValue<AppItem | null>("apps", "selectedApp", null);
  const [search, setSearch] = useToolWindowValue("apps", "search", "");
  const [filterKind, setFilterKind] = useToolWindowValue<"all" | "user" | "system">("apps", "filterKind", "all");
  useToolDetachGuard("apps", isRefreshing);
  const [detailsNonce, setDetailsNonce] = useState(0);
  const appRequestRef = useRef(0);
  const serialRef = useRef(selectedSerial);
  serialRef.current = selectedSerial;

  const showToast = (message: string) => {
    notify(message, "info");
  };

  const [confirmModal, setConfirmModal] = useState<{
    open: boolean;
    title: string;
    message: string;
    action: () => void;
    confirmLabel?: string;
    variant?: "danger" | "primary";
  }>({
    open: false,
    title: "",
    message: "",
    action: () => {},
  });

  const noDevice = !tauri || !selectedSerial;

  const restoredAppRef = useRef(readToolWindowState<AppItem>("apps-selectedApp"));
  const loadApps = async (targetSerial: string) => {
    const request = ++appRequestRef.current;
    setApps([]);
    setSelectedApp(null);
    if (!targetSerial) {
      setApps([]);
      setSelectedApp(null);
      return;
    }
    const raw = await invoke<ListedApplication[]>("list_adb_applications", { serial: targetSerial });
    if (request !== appRequestRef.current || serialRef.current !== targetSerial) return;
    const mapped = raw.map(toAppItem);
    setApps(mapped);
    const selectedPackage = restoredAppRef.current?.packageName;
    restoredAppRef.current = undefined;
    setSelectedApp((current) => mapped.find((item) => item.packageName === (current?.packageName || selectedPackage)) ?? mapped[0] ?? null);
    setDetailsNonce((nonce) => nonce + 1);
  };

  const handleRefreshApps = async () => {
    if (!tauri) {
      notifyDesktopRequired();
      return;
    }
    setIsRefreshing(true);
    try {
      const list = await listAdbDevices();
      const online = list.filter((item) => item.state === "device").map(toDeviceOption);
      setDevices(online);
      if (online.length === 0) {
        setSelectedSerial("");
        setApps([]);
        setSelectedApp(null);
        showToast("未检测到在线设备");
        return;
      }
      const next = online.some((item) => item.serial === selectedSerial) ? selectedSerial : online[0].serial;
      serialRef.current = next;
      setSelectedSerial(next);
      await loadApps(next);
    } catch (reason) {
      setApps([]);
      setSelectedApp(null);
      notify(formatInvokeError(reason), "error");
    } finally {
      setIsRefreshing(false);
    }
  };

  useEffect(() => {
    if (!tauri) return;
    void handleRefreshApps();
  }, [tauri]);

  useEffect(() => {
    if (!tauri || !selectedSerial || !selectedApp) return;
    const packageName = selectedApp.packageName;
    let cancelled = false;
    (async () => {
      try {
        const details = await invoke<PackageDetails>("get_adb_package_details", {
          serial: selectedSerial,
          packageName,
        });
        if (cancelled) return;
        const patch: Partial<AppItem> = {
          versionName: details.versionName || "-",
          versionCode: details.versionCode || "-",
          uid: details.uid || "-",
          enabled: details.enabledState ?? (details.enabled === "true" ? true : details.enabled === "false" ? false : null),
          permissions: details.permissions ?? (details.requestedPermissions ?? []).map((name) => ({ name, granted: null })),
        };
        if (details.sourcePaths?.[0]) patch.sourcePath = details.sourcePaths[0];
        setApps((prev) => prev.map((item) => (item.packageName === packageName ? { ...item, ...patch } : item)));
        setSelectedApp((prev) => (prev && prev.packageName === packageName ? { ...prev, ...patch } : prev));
      } catch (reason) {
        if (!cancelled) notify(formatInvokeError(reason), "error");
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [tauri, selectedSerial, selectedApp?.packageName, detailsNonce]);

  const applySerial = (next: string) => {
    serialRef.current = next;
    setSelectedSerial(next);
    if (!tauri) return;
    setIsRefreshing(true);
    void loadApps(next)
      .catch((reason) => showToast(formatInvokeError(reason)))
      .finally(() => setIsRefreshing(false));
  };

  useApplicationPageAction("open.tool.apps", (action) => {
    const targetSerial = action.parameters.serial || selectedSerial;
    serialRef.current = targetSerial;
    const packageName = action.parameters.packageName || "";
    setSearch(packageName);
    publishAiTaskActivity({ running: true, status: "running", title: "正在加载应用包", detail: packageName ? `正在定位 ${packageName}。` : "正在读取设备应用列表。", actionId: action.actionId, progress: 15 });
    if (!tauri) {
      notifyDesktopRequired(action.actionId);
      return;
    }
    if (!targetSerial) {
      const message = "未检测到在线设备。";
      notify(message, "warning");
      publishAiTaskActivity({ running: false, status: "error", title: "应用包加载失败", detail: message, actionId: action.actionId });
      return;
    }
    setSelectedSerial(targetSerial);
    const request = ++appRequestRef.current;
    setApps([]);
    setSelectedApp(null);
    setIsRefreshing(true);
    void invoke<ListedApplication[]>("list_adb_applications", { serial: targetSerial })
      .then((raw) => {
        if (request !== appRequestRef.current || serialRef.current !== targetSerial) return;
        const mapped = raw.map(toAppItem);
        const selected = mapped.find((app) => app.packageName === packageName) || mapped[0] || null;
        setApps(mapped);
        setSelectedApp(selected);
        setDetailsNonce((nonce) => nonce + 1);
        publishAiTaskActivity({ running: false, status: "done", title: "应用包已加载", detail: selected ? `已定位 ${selected.packageName}。` : "设备没有可读取的应用包。", actionId: action.actionId, progress: 100 });
      })
      .catch((reason) => {
        if (request !== appRequestRef.current || serialRef.current !== targetSerial) return;
        const message = formatInvokeError(reason);
        notify(message, "error");
        publishAiTaskActivity({ running: false, status: "error", title: "应用包加载失败", detail: message, actionId: action.actionId });
      })
      .finally(() => { if (request === appRequestRef.current) setIsRefreshing(false); });
  });

  const counts = useMemo(() => {
    const thirdCount = apps.filter((a) => a.kind === "第三方").length;
    const sysCount = apps.filter((a) => a.kind === "系统内置").length;
    return { all: apps.length, third: thirdCount, sys: sysCount };
  }, [apps]);

  const filteredApps = useMemo(() => {
    return apps.filter((app) => {
      if (filterKind === "user" && app.kind !== "第三方") return false;
      if (filterKind === "system" && app.kind !== "系统内置") return false;
      const q = search.trim().toLowerCase();
      if (!q) return true;
      return (
        app.packageName.toLowerCase().includes(q) ||
        app.sourcePath.toLowerCase().includes(q)
      );
    });
  }, [apps, search, filterKind]);

  const handleInstallApk = async () => {
    if (!tauri) {
      notifyDesktopRequired();
      return;
    }
    if (!selectedSerial) {
      showToast("未检测到在线设备");
      return;
    }
    try {
      const apkPath = await openFile("选择 APK", [{ name: "APK", extensions: ["apk"] }]);
      if (!apkPath) return;
      const message = await invoke<string>("install_adb_package", {
        request: { serial: selectedSerial, apkPath, replace: true, allowTest: false },
      });
      showToast(message.trim() ? message : "安装完成");
      await loadApps(selectedSerial);
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    }
  };

  const runPackageAction = async (action: PackageAction, packageName: string, serialValue: string) => {
    return invoke<string>("run_adb_package_action", {
      serial: serialValue,
      packageName,
      action,
    });
  };

  const handleLaunch = async () => {
    if (!selectedApp) return;
    if (!tauri) {
      notifyDesktopRequired();
      return;
    }
    if (!selectedSerial) {
      showToast("未检测到在线设备");
      return;
    }
    try {
      const message = await runPackageAction("launch", selectedApp.packageName, selectedSerial);
      showToast(message.trim() ? message : `已启动 ${selectedApp.packageName}`);
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    }
  };

  const { confirmAction, confirmationDialog } = useConfirmAction();
  const executeForceStop = async () => {
    if (!selectedApp) return;
    if (!tauri) {
      notifyDesktopRequired();
      return;
    }
    if (!selectedSerial) {
      showToast("未检测到在线设备");
      return;
    }
    try {
      const message = await runPackageAction("force-stop", selectedApp.packageName, selectedSerial);
      showToast(message.trim() ? message : `已强制停止 ${selectedApp.packageName}`);
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    }
  };

  const handleClearData = () => {
    if (!selectedApp) return;
    const packageName = selectedApp.packageName;
    const serialValue = selectedSerial;
    setConfirmModal({
      open: true,
      title: "清除应用数据",
      message: `确定要清除「${packageName}」的所有内部数据、数据库与缓存吗？此操作无法撤销。`,
      confirmLabel: "确认清除",
      variant: "danger",
      action: () => {
        if (!tauri) {
      notifyDesktopRequired();
      return;
    }
        void (async () => {
          try {
            const message = await runPackageAction("clear-data", packageName, serialValue);
            showToast(message.trim() ? message : `已清除 ${packageName}`);
          } catch (reason) {
            notify(formatInvokeError(reason), "error");
          }
        })();
      },
    });
  };

  const executeToggleEnable = async () => {
    if (!selectedApp) return;
    if (!tauri) {
      notifyDesktopRequired();
      return;
    }
    if (!selectedSerial) {
      showToast("未检测到在线设备");
      return;
    }
    const packageName = selectedApp.packageName;
    if (selectedApp.enabled === null) { notify("应用启用状态未检测，无法切换。", "warning"); return; }
    const nextEnabled = !selectedApp.enabled;
    const action: PackageAction = nextEnabled ? "enable" : "disable";
    try {
      const message = await runPackageAction(action, packageName, selectedSerial);
      setApps((prev) => prev.map((item) => (item.packageName === packageName ? { ...item, enabled: nextEnabled } : item)));
      setSelectedApp((prev) => (prev && prev.packageName === packageName ? { ...prev, enabled: nextEnabled } : prev));
      showToast(message.trim() ? message : `已${nextEnabled ? "启用" : "停用"}应用 ${packageName}`);
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    }
  };

  const handleForceStop = () => {
    if (!selectedApp) return;
    confirmAction({ title: "强制停止应用", message: `确认强制停止「${selectedApp.packageName}」？未保存的数据可能丢失。`, confirmLabel: "确认停止", onConfirm: executeForceStop });
  };
  const handleToggleEnable = () => {
    if (!selectedApp) return;
    if (selectedApp.enabled === false) { void executeToggleEnable(); return; }
    if (selectedApp.enabled === null) { notify("应用启用状态未检测，无法切换。", "warning"); return; }
    confirmAction({ title: "停用应用", message: `确认停用「${selectedApp.packageName}」？应用将无法启动，停用系统应用可能影响设备功能。`, confirmLabel: "确认停用", onConfirm: executeToggleEnable });
  };

  const handleExportApk = async () => {
    if (!selectedApp) return;
    if (!tauri) {
      notifyDesktopRequired();
      return;
    }
    if (!selectedSerial) {
      showToast("未检测到在线设备");
      return;
    }
    try {
      const destination = await openDirectory("选择导出目录");
      if (!destination) return;
      const saved = await invoke<string[]>("export_adb_package", {
        request: { serial: selectedSerial, packageName: selectedApp.packageName, destination },
      });
      showToast(saved.length ? saved.join("\n") : "已导出");
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    }
  };

  const handleUninstall = () => {
    if (!selectedApp) return;
    const packageName = selectedApp.packageName;
    const serialValue = selectedSerial;
    setConfirmModal({
      open: true,
      title: "卸载应用包",
      message: `确定要从设备中彻底卸载「${packageName}」吗？`,
      confirmLabel: "确认卸载",
      variant: "danger",
      action: () => {
        if (!tauri) {
      notifyDesktopRequired();
      return;
    }
        void (async () => {
          try {
            const message = await runPackageAction("uninstall", packageName, serialValue);
            showToast(message.trim() ? message : `应用 ${packageName} 已卸载`);
            await loadApps(serialValue);
          } catch (reason) {
            notify(formatInvokeError(reason), "error");
          }
        })();
      },
    });
  };

  const appProperties: PropertyItem[] = useMemo(() => {
    if (!selectedApp) return [];
    return [
      {
        icon: <Layers size={14} />,
        label: "应用版本",
        value: `${selectedApp.versionName} (${selectedApp.versionCode})`,
      },
      {
        icon: <Cpu size={14} />,
        label: "系统 UID",
        value: selectedApp.uid,
      },
      {
        icon: <Activity size={14} />,
        label: "运行状态",
        value: selectedApp.enabled === null ? "未检测" : selectedApp.enabled ? "已启用" : "已停用",
        valueType: selectedApp.enabled === null ? "unknown" : selectedApp.enabled ? "green" : "orange",
      },
      {
        icon: <FileCode size={14} />,
        label: "安装类型",
        value: selectedApp.kind,
      },
    ];
  }, [selectedApp]);

  const grantedCount = useMemo(() => {
    if (!selectedApp) return 0;
    return selectedApp.permissions.filter((p) => p.granted === true).length;
  }, [selectedApp]);

  const onlineDevices = devices.filter((item) => item.state === "device");

  return (
    <div className="pm-container">

      <header className="pm-top-header">
        <div className="pm-header-left">
          <ToolBackButton onBack={onBack} />
          <div className="pm-title-wrap">
            <span className="pm-title">应用包管理器 (Package Manager)</span>
            <span className="pm-subtitle">· 深度包名管理、提取导出、状态控制与权限分析</span>
          </div>
        </div>
        {tauri && (
          <div style={{ width: 240, flexShrink: 0 }}>
            <Select
              size="sm"
              value={selectedSerial}
              placeholder="未检测到在线设备"
              disabled={onlineDevices.length === 0}
              options={onlineDevices.map((item) => ({ label: item.label, value: item.serial }))}
              onChange={applySerial}
            />
          </div>
        )}
      </header>

      <div className="pm-content">
        <div className="pm-action-strip">
          <div className="pm-action-titles">
            <h1>已安装应用</h1>
            <p>查看包信息、导出 APK、安装或控制当前设备上的应用。</p>
          </div>
          <div className="pm-action-buttons">
            <Button
              size="sm"
              variant="primary"
              onClick={() => void handleInstallApk()}
              disabled={noDevice}
              icon={<Upload size={14} />}
            >
              安装 APK
            </Button>
            <Button
              size="sm"
              variant="secondary"
              onClick={() => void handleRefreshApps()}
              disabled={isRefreshing}
              icon={<RefreshCw size={13} className={isRefreshing ? "animate-spin" : ""} />}
            >
              检测设备
            </Button>
          </div>
        </div>

        <div className="pm-devices-bar">
          <span className="pm-devices-label">当前设备:</span>
          {devices.map((device) => {
            const isSelected = selectedSerial === device.serial;
            return (
              <div
                key={device.serial}
                className={`pm-device-card ${isSelected ? "active" : ""}`}
                onClick={() => applySerial(device.serial)}
                role="button"
                tabIndex={0}
              >
                <div className="pm-device-icon-box">
                  <Smartphone size={16} />
                </div>
                <div className="pm-device-info">
                  <span className="pm-device-name">{device.model}</span>
                  <span className="pm-device-serial">{device.serial}</span>
                </div>
                <Badge variant={isSelected ? "success" : "neutral"} pulse={isSelected}>
                  {isSelected ? "在线" : "就绪"}
                </Badge>
              </div>
            );
          })}
        </div>

        <div className="pm-workspace">
          <div className="pm-list-card">
            <div className="pm-list-header">
              <div className="pm-list-toolbar-row">
                <div className="pm-search-wrap">
                  <Input
                    size="sm"
                    prefixIcon={<Search size={14} />}
                    value={search}
                    onChange={(e) => setSearch(e.target.value)}
                    placeholder="搜索包名或路径..."
                    clearable
                    onClear={() => setSearch("")}
                  />
                </div>
                <Button
                  size="sm"
                  variant="secondary"
                  onClick={() => void handleRefreshApps()}
                  disabled={isRefreshing}
                  title="重新扫描包列表"
                  style={{ padding: "0 10px" }}
                >
                  <RefreshCw size={13} className={isRefreshing ? "animate-spin" : ""} />
                </Button>
              </div>

              <div className="pm-segmented-wrap">
                <SegmentedControl value={filterKind} onChange={setFilterKind} label="应用类型" options={[
                  { value: "all", label: `全部应用 (${counts.all})` }, { value: "user", label: `第三方 (${counts.third})` },
                  { value: "system", label: `系统内置 (${counts.sys})` },
                ]} />
              </div>
            </div>

            <div className="pm-rows-container">
              {filteredApps.map((app) => {
                const isCurrent = selectedApp?.packageName === app.packageName;
                return (
                  <div
                    key={app.packageName}
                    className={`pm-app-row ${isCurrent ? "active" : ""}`}
                    onClick={() => setSelectedApp(app)}
                    role="button"
                    tabIndex={0}
                  >
                    <div className="pm-app-icon-wrap">
                      <Package size={15} />
                    </div>
                    <div className="pm-app-info">
                      <span className="pm-app-name">{app.packageName}</span>
                      <span className="pm-app-path">{app.sourcePath}</span>
                    </div>
                    <Badge variant={app.kind === "第三方" ? "primary" : "neutral"}>
                      {app.kind}
                    </Badge>
                  </div>
                );
              })}

              {filteredApps.length === 0 && (
                <div className="pm-empty-placeholder">
                  <Package size={28} />
                  <span>没有匹配的应用包</span>
                </div>
              )}
            </div>
          </div>

          {selectedApp ? (
            <div className="pm-detail-card">
              <div className="pm-detail-card-header">
                <div className="pm-detail-title-group">
                  <small>当前选定应用包</small>
                  <h2>{selectedApp.packageName}</h2>
                </div>
                <Button
                  size="sm"
                  variant="secondary"
                  onClick={() => onOpenTerminal?.(selectedSerial)}
                  title="在终端中调试该应用"
                  icon={<Terminal size={14} />}
                >
                  去终端
                </Button>
              </div>

              <div className="pm-detail-card-body">
                <div className="pm-properties-wrapper">
                  <PropertyGrid
                    items={appProperties}
                    columns={2}
                    style={{
                      border: "none",
                      boxShadow: "none",
                      background: "transparent",
                      padding: 0,
                    }}
                  />
                </div>

                <div className="pm-path-card">
                  <div className="pm-path-card-head">
                    <div className="pm-path-label-wrap">
                      <HardDrive size={13} style={{ color: "var(--accent-primary)" }} />
                      <span>APK 安装包物理路径</span>
                    </div>
                    <button
                      type="button"
                      className="pm-path-copy-btn"
                      onClick={() => {
                        navigator.clipboard?.writeText(selectedApp.sourcePath);
                        showToast(`已复制路径到剪贴板: ${selectedApp.sourcePath}`);
                      }}
                      title="复制完整路径"
                    >
                      <Copy size={11} />
                      <span>复制</span>
                    </button>
                  </div>
                  <div className="pm-path-value">{selectedApp.sourcePath}</div>
                </div>

                <Divider />

                <div className="pm-action-group">
                  <div className="pm-action-label">应用状态与生命周期控制</div>
                  <div className="pm-actions-bar">
                    <Button
                      size="sm"
                      variant="primary"
                      onClick={() => void handleLaunch()}
                      disabled={noDevice}
                      icon={<Play size={13} fill="currentColor" />}
                    >
                      启动应用
                    </Button>

                    <Button
                      size="sm"
                      variant="secondary"
                      onClick={() => void handleForceStop()}
                      disabled={noDevice}
                      icon={<Square size={13} fill="currentColor" />}
                    >
                      强制停止
                    </Button>

                    <Button
                      size="sm"
                      variant="secondary"
                      onClick={() => void handleToggleEnable()}
                      disabled={noDevice}
                      icon={<Power size={13} />}
                    >
                        {selectedApp.enabled === null ? "状态未检测" : selectedApp.enabled ? "停用应用" : "启用应用"}
                    </Button>

                    <Button
                      size="sm"
                      variant="secondary"
                      onClick={handleClearData}
                      disabled={noDevice}
                      icon={<Trash2 size={13} />}
                    >
                      清除数据
                    </Button>

                    <Button
                      size="sm"
                      variant="secondary"
                      onClick={() => void handleExportApk()}
                      disabled={noDevice}
                      icon={<Download size={13} />}
                    >
                      导出 APK
                    </Button>

                    <Button
                      size="sm"
                      variant="danger"
                      onClick={handleUninstall}
                      disabled={noDevice}
                      icon={<Trash2 size={13} />}
                    >
                      彻底卸载
                    </Button>
                  </div>
                </div>

                <Divider />

                <div className="pm-permissions-section">
                  <div className="pm-permissions-head">
                    <div className="pm-permissions-head-left">
                      <ShieldCheck size={14} style={{ color: "var(--accent-primary)" }} />
                      <span>已声明的系统权限 (Declared Permissions)</span>
                    </div>
                    <div style={{ display: "flex", gap: 6 }}>
                      <Badge variant={grantedCount ? "success" : "neutral"}>
                        已授权 {grantedCount}
                      </Badge>
                      <Badge variant={selectedApp.permissions.some((permission) => permission.granted === false) ? "danger" : "neutral"}>
                        未授权 {selectedApp.permissions.filter((permission) => permission.granted === false).length}
                      </Badge>
                      {selectedApp.permissions.some((permission) => permission.granted === null) && <Badge variant="neutral">未检测 {selectedApp.permissions.filter((permission) => permission.granted === null).length}</Badge>}
                    </div>
                  </div>

                  <div className="pm-permissions-scroll-box">
                    {selectedApp.permissions.map((perm) => (
                      <div key={perm.name} className="pm-permission-item">
                        <div className="pm-permission-left">
                          <ShieldCheck size={12} className="pm-permission-icon" />
                          <span className="pm-permission-name">{perm.name}</span>
                        </div>
                        <Badge
                          variant={perm.granted === true ? "success" : perm.granted === false ? "danger" : "neutral"}
                          pulse={perm.granted === true}
                        >
                          {perm.granted === true ? "已授权" : perm.granted === false ? "未授权" : "未检测"}
                        </Badge>
                      </div>
                    ))}
                  </div>
                </div>
              </div>
            </div>
          ) : (
            <div className="pm-detail-card pm-empty-placeholder">
              <Package size={36} />
              <span>请选择左侧的一个应用包以查看详情和操作</span>
            </div>
          )}
        </div>
      </div>

      <ConfirmDialog
        open={confirmModal.open}
        title={confirmModal.title}
        message={confirmModal.message}
        confirmLabel={confirmModal.confirmLabel}
        variant={confirmModal.variant}
        onCancel={() => setConfirmModal((prev) => ({ ...prev, open: false }))}
        onConfirm={() => {
          confirmModal.action();
          setConfirmModal((prev) => ({ ...prev, open: false }));
        }}
      />
      {confirmationDialog}
    </div>
  );
};
