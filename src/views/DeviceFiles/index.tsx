import { ToolBackButton } from "@/components/ui/ToolBackButton";
import { readToolWindowState, useToolWindowValue } from "@/platform/toolWindowState";
import React, { useEffect, useRef, useState } from "react";
import {
  Folder,
  File,
  FileText,
  Upload,
  Download,
  Trash2,
  RefreshCw,
  FolderPlus,
  ShieldCheck,
  HardDrive,
  Save,
  CheckCircle2,
} from "lucide-react";
import { Button, Badge, Select } from "@/components/ui";
import { useConfirmAction } from "@/components/ConfirmDialog/useConfirmAction";
import { PropertyGrid, type PropertyItem } from "@/components/ui/PropertyGrid";
import { ToolEmptyState } from "@/components/ui/ToolEmptyState";
import {
  deviceLabel,
  deviceOptionLabel,
  formatInvokeError,
  invoke,
  isTauri,
  listAdbDevices,
  openDirectory,
  openFile,
  type AdbDevice,
} from "@/platform/runtime";
import { notify, publishAiTaskActivity, notifyDesktopRequired } from "@/platform/notifications";
import { useApplicationPageAction } from "@/application/useApplicationPageAction";
import "./device-files.css";

interface DeviceFileItem {
  name: string;
  path: string;
  isDirectory: boolean;
  size?: number;
  modified: string;
}

interface RemoteFilePayload {
  name: string;
  path: string;
  isDirectory: boolean;
  size?: number | null;
  modified?: string | null;
}

const TEXT_FILE = /\.(prop|txt|xml|json|conf|cfg|rc|sh|log)$/i;

function normalizeFile(item: RemoteFilePayload): DeviceFileItem {
  return {
    name: item.name,
    path: item.path,
    isDirectory: item.isDirectory,
    size: item.size ?? undefined,
    modified: item.modified ?? "",
  };
}

export const DeviceFilesView: React.FC<{ onBack?: () => void }> = ({ onBack }) => {
  const { confirmAction, confirmationDialog } = useConfirmAction();
  const tauri = isTauri();
  const [devices, setDevices] = useState<AdbDevice[]>([]);
  const [serial, setSerial] = useToolWindowValue("files", "serial", "");
  const [currentPath, setCurrentPath] = useToolWindowValue("files", "currentPath", "/sdcard");
  const [selectedFile, setSelectedFile] = useToolWindowValue<DeviceFileItem | null>("files", "selectedFile", null);
  const [editorContent, setEditorContent] = useToolWindowValue("files", "editorContent", "");
  const [isRoot, setIsRoot] = useState<boolean | null>(null);
  const [storage, setStorage] = useState<{ totalBytes: number; availableBytes: number } | null>(null);
  const [files, setFiles] = useState<DeviceFileItem[]>(() => []);
  const [reloadTick, setReloadTick] = useState(0);
  const selectedPathRef = useRef<string | null>(readToolWindowState<DeviceFileItem>("files-selectedFile")?.path || null);
  const serialRef = useRef(serial);
  serialRef.current = serial;
  const announceRefresh = useRef(false);
  const pendingAiActionRef = useRef<string | null>(null);

  const showToast = (msg: string) => {
    notify(msg, "info");
  };

  const online = devices.filter((item) => item.state === "device");
  const noDevice = !tauri || !serial;

  const refreshDevices = async () => {
    if (!tauri) { notifyDesktopRequired(); return; }
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
    if (!tauri) return;
    setFiles([]);
    const announce = announceRefresh.current;
    announceRefresh.current = false;
    let cancelled = false;
    (async () => {
      if (!serial) {
        setFiles([]);
        return;
      }
      try {
        const items = await invoke<RemoteFilePayload[]>("list_adb_remote_files", {
          serial,
          path: currentPath,
        });
        if (cancelled) return;
        setFiles(items.map(normalizeFile));
        if (announce) showToast("已刷新目录内容");
        if (pendingAiActionRef.current) {
          publishAiTaskActivity({ running: false, status: "done", title: "设备目录已加载", detail: `已读取 ${currentPath}，共 ${items.length} 项。`, actionId: pendingAiActionRef.current, progress: 100 });
          pendingAiActionRef.current = null;
        }
      } catch (reason) {
        if (cancelled) return;
        setFiles([]);
        const message = formatInvokeError(reason);
        notify(message, "error");
        if (pendingAiActionRef.current) {
          publishAiTaskActivity({ running: false, status: "error", title: "设备目录加载失败", detail: message, actionId: pendingAiActionRef.current });
          pendingAiActionRef.current = null;
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [tauri, serial, currentPath, reloadTick]);

  const previousSerialRef = useRef(readToolWindowState<string>("files-serial"));
  useEffect(() => {
    setStorage(null);
    setIsRoot(null);
    if (previousSerialRef.current !== serial || reloadTick > 0) {
      selectedPathRef.current = null;
      setSelectedFile(null);
      setEditorContent("");
    }
    previousSerialRef.current = serial;
    if (!tauri || !serial) return;
    let cancelled = false;
    void invoke<{ totalBytes: number; availableBytes: number }>("get_adb_storage_info", { serial, path: "/data" })
      .then((value) => { if (!cancelled) setStorage(value); })
      .catch(() => { if (!cancelled) setStorage(null); });
    void invoke<{ isRoot: boolean }>("get_adb_privilege_status", { serial })
      .then((value) => { if (!cancelled) setIsRoot(value.isRoot); })
      .catch(() => { if (!cancelled) setIsRoot(null); });
    return () => { cancelled = true; };
  }, [tauri, serial, reloadTick]);

  const crumbs = currentPath.split("/").filter(Boolean);

  const handleCrumbClick = (index: number) => {
    const next = "/" + crumbs.slice(0, index + 1).join("/");
    setCurrentPath(next);
    if (tauri) {
      selectedPathRef.current = null;
      setSelectedFile(null);
    } else {
      showToast(`切换路径：${next}`);
    }
  };

  const handleRowClick = (item: DeviceFileItem) => {
    if (item.isDirectory) {
      setCurrentPath(item.path);
      selectedPathRef.current = null;
      setSelectedFile(null);
      if (!tauri) showToast(`进入目录：${item.name}`);
      return;
    }
    selectedPathRef.current = item.path;
    setSelectedFile(item);
    if (!tauri) {
      notifyDesktopRequired();
      return;
    }
    if (!TEXT_FILE.test(item.name)) {
      setEditorContent("");
      return;
    }
    if (!serial) {
      showToast("未检测到在线设备");
      return;
    }
    setEditorContent("");
    void (async () => {
      try {
        const text = await invoke<{ content: string; size: number }>("read_adb_remote_text", {
          serial,
          path: item.path,
        });
        if (selectedPathRef.current !== item.path || serialRef.current !== serial) return;
        setEditorContent(text.content);
        setSelectedFile((current) =>
          current && current.path === item.path ? { ...current, size: text.size } : current
        );
      } catch (reason) {
        if (selectedPathRef.current !== item.path || serialRef.current !== serial) return;
        notify(formatInvokeError(reason), "error");
      }
    })();
  };

  const handleRefresh = async () => {
    if (!tauri) {
      notifyDesktopRequired();
      return;
    }
    await refreshDevices();
    announceRefresh.current = true;
    setReloadTick((tick) => tick + 1);
  };

  useApplicationPageAction("open.tool.files", (action) => {
    const destination = action.parameters.path || currentPath;
    if (action.parameters.serial) setSerial(action.parameters.serial);
    if (!tauri) {
      notifyDesktopRequired(action.actionId);
      return;
    }
    pendingAiActionRef.current = action.actionId;
    announceRefresh.current = false;
    setCurrentPath(destination);
    setReloadTick((tick) => tick + 1);
    publishAiTaskActivity({ running: true, status: "running", title: "正在读取设备目录", detail: `正在加载 ${destination}。`, actionId: action.actionId, progress: 15 });
  });

  const handleRoot = async () => {
    if (!tauri) {
      notifyDesktopRequired();
      return;
    }
    if (!serial) {
      showToast("未检测到在线设备");
      return;
    }
    try {
      const privilege = await invoke<{ isRoot: boolean }>("request_adb_root", { serial });
      setIsRoot(privilege.isRoot);
      showToast(privilege.isRoot ? "adbd 已是 Root" : "adbd 未获得 Root");
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    }
  };

  const handleCreateDirectory = async () => {
    if (!tauri) {
      notifyDesktopRequired();
      return;
    }
    if (!serial) {
      showToast("未检测到在线设备");
      return;
    }
    const name = window.prompt("目录名称");
    if (!name || !name.trim()) return;
    try {
      await invoke("create_adb_directory", {
        request: { serial, path: `${currentPath.replace(/\/$/, "")}/${name.trim()}` },
      });
      showToast(`已创建目录 ${name.trim()}`);
      setReloadTick((tick) => tick + 1);
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    }
  };

  const handleUpload = async () => {
    if (!tauri) {
      notifyDesktopRequired();
      return;
    }
    if (!serial) {
      showToast("未检测到在线设备");
      return;
    }
    try {
      const localPath = await openFile("选择要推送的文件");
      if (!localPath) return;
      const result = await invoke<string>("push_adb_file", {
        request: { serial, remotePath: currentPath, localPath },
      });
      showToast(result.trim() ? result : "已推送到设备");
      setReloadTick((tick) => tick + 1);
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    }
  };

  const handleDownload = async (item: DeviceFileItem) => {
    if (!tauri) {
      notifyDesktopRequired();
      return;
    }
    if (!serial) {
      showToast("未检测到在线设备");
      return;
    }
    try {
      const destination = await openDirectory("选择保存目录");
      if (!destination) return;
      const result = await invoke<string>("pull_adb_file", {
        request: { serial, remotePath: item.path, localPath: destination },
      });
      showToast(result.trim() ? result : destination);
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    }
  };

  const handleDelete = (item: DeviceFileItem) => {
    if (!tauri || !serial) {
      showToast("未检测到在线设备");
      return;
    }
    confirmAction({ title: item.isDirectory ? "删除设备目录" : "删除设备文件", message: `确认从设备 ${serial} 删除「${item.path}」${item.isDirectory ? "及其中的全部文件" : ""}？此操作无法恢复。`, onConfirm: async () => {
      if (!tauri) setFiles((current) => current.filter((file) => file.path !== item.path));
      else {
        await invoke("delete_adb_remote_path", {
          request: { serial, path: item.path, isDirectory: item.isDirectory, confirmed: true },
        });
        setReloadTick((tick) => tick + 1);
      }
      if (selectedPathRef.current === item.path) { selectedPathRef.current = null; setSelectedFile(null); }
      showToast(`已删除 ${item.name}`);
    } });
  };

  const handleSave = async () => {
    if (!tauri) {
      notifyDesktopRequired();
      return;
    }
    if (!serial) {
      showToast("未检测到在线设备");
      return;
    }
    if (!selectedFile) return;
    if (!TEXT_FILE.test(selectedFile.name)) {
      showToast("仅支持保存文本文件");
      return;
    }
    try {
      await invoke("write_adb_remote_text", {
        request: {
          serial,
          path: selectedFile.path,
          content: editorContent,
          confirmProtected: false,
        },
      });
      showToast("已写入设备文件");
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    }
  };

  const formatSize = (bytes?: number) => {
    if (bytes === undefined) return "--";
    if (bytes === 0) return "0 B";
    if (bytes >= 1024 * 1024 * 1024) return (bytes / (1024 * 1024 * 1024)).toFixed(1) + " GB";
    if (bytes >= 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + " MB";
    return Math.max(1, Math.ceil(bytes / 1024)) + " KB";
  };

  const storageItems: PropertyItem[] = [
    { icon: <HardDrive size={14} />, label: "总存储空间", value: storage ? formatSize(storage.totalBytes) : "未检测", valueType: storage ? "default" : "unknown" },
    { icon: <CheckCircle2 size={14} />, label: "可用空间", value: storage ? formatSize(storage.availableBytes) : "未检测", valueType: storage ? "green" : "unknown" },
    { icon: <ShieldCheck size={14} />, label: "Root 访问权限", value: isRoot === null ? "未检测" : isRoot ? "adbd Root" : "受限 Shell", valueType: isRoot === null ? "unknown" : isRoot ? "green" : "orange" },
    { icon: <Folder size={14} />, label: "当前条目数量", value: `${files.length} 项` },
  ];

  return (
    <div className="device-files-container">
      {confirmationDialog}

      {/* 顶部 Header */}
      <header className="device-files-header">
        <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <ToolBackButton onBack={onBack} />
          <h2 style={{ margin: 0, fontSize: 15, fontWeight: 700, color: "var(--text-primary)" }}>
            设备存储与文件互传
          </h2>
          <Badge variant={isRoot ? "success" : "neutral"}>
            {isRoot === null ? "权限未检测" : isRoot ? "Root 提权模式" : "标准 Shell 模式"}
          </Badge>
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
            icon={<ShieldCheck size={13} />}
            disabled={noDevice}
            onClick={() => void handleRoot()}
          >
            获取 Root 提权
          </Button>
          <Button
            variant="secondary"
            size="sm"
            icon={<FolderPlus size={13} />}
            disabled={noDevice}
            onClick={() => void handleCreateDirectory()}
          >
            新建目录
          </Button>
          <Button
            variant="primary"
            size="sm"
            icon={<Upload size={13} />}
            disabled={noDevice}
            onClick={() => void handleUpload()}
          >
            上传到设备
          </Button>
        </div>
      </header>

      {/* 面包屑导航栏 */}
      <div className="device-files-path-bar">
        <div className="device-files-breadcrumbs custom-scrollbar">
          <button
            type="button"
            className={`device-files-crumb ${crumbs.length === 0 ? "active" : ""}`}
            onClick={() => {
              setCurrentPath("/");
              if (tauri) {
                selectedPathRef.current = null;
                setSelectedFile(null);
              }
            }}
          >
            / (根目录)
          </button>
          {crumbs.map((crumb, idx) => (
            <React.Fragment key={idx}>
              <span style={{ color: "var(--text-tertiary)", fontSize: 11 }}>/</span>
              <button
                type="button"
                className={`device-files-crumb ${idx === crumbs.length - 1 ? "active" : ""}`}
                onClick={() => handleCrumbClick(idx)}
              >
                {crumb}
              </button>
            </React.Fragment>
          ))}
        </div>

        <Button
          variant="subtle"
          size="xs"
          icon={<RefreshCw size={12} />}
          onClick={() => void handleRefresh()}
        >
          刷新
        </Button>
      </div>

      {/* 主体双栏内容 */}
      <div className="device-files-body">
        {/* 左侧：文件表格列表 */}
        <div className="device-files-list-col">
          <div className="device-files-list-header">
            <span>名称</span>
            <span>大小</span>
            <span>修改时间</span>
            <span style={{ textAlign: "right" }}>快捷操作</span>
          </div>

          <div className="device-files-scroll-list custom-scrollbar">
            {files.map((item) => (
              <div
                key={item.path}
                className={`device-files-row ${selectedFile?.path === item.path ? "selected" : ""}`}
                onClick={() => handleRowClick(item)}
              >
                <div className="device-files-name-cell">
                  {item.isDirectory ? (
                    <Folder size={15} color="var(--warning-text)" />
                  ) : item.name.endsWith(".apk") ? (
                    <File size={15} color="var(--success-text)" />
                  ) : item.name.endsWith(".png") ? (
                    <File size={15} color="var(--accent-primary)" />
                  ) : (
                    <FileText size={15} color="var(--control-text)" />
                  )}
                  <span>{item.name}</span>
                </div>
                <span>{formatSize(item.size)}</span>
                <span>{item.modified}</span>
                <div className="device-files-actions-cell" style={{ justifyContent: "flex-end" }}>
                  {!item.isDirectory && (
                    <Button
                      variant="subtle"
                      size="xs"
                      icon={<Download size={12} />}
                      disabled={noDevice}
                      onClick={(e) => {
                        e.stopPropagation();
                        void handleDownload(item);
                      }}
                    />
                  )}
                  <Button
                    variant="subtle"
                    size="xs"
                    icon={<Trash2 size={12} />}
                    title={`删除 ${item.name}`}
                    aria-label={`删除 ${item.name}`}
                    disabled={noDevice}
                    onClick={(e) => {
                      e.stopPropagation();
                      void handleDelete(item);
                    }}
                  />
                </div>
              </div>
            ))}
          </div>
        </div>

        {/* 右侧：文件属性与在线编辑 */}
        <aside className="device-files-aside-panel custom-scrollbar">
              <PropertyGrid items={storageItems} columns={1} />
              {noDevice && <ToolEmptyState compact title="等待连接设备" description="连接 ADB 设备后读取存储容量和权限。" />}

          {selectedFile ? (
            <div className="device-files-preview-box">
              <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
                <strong style={{ fontSize: 12.5, color: "var(--text-primary)" }}>{selectedFile.name}</strong>
                <Badge variant="neutral">{formatSize(selectedFile.size)}</Badge>
              </div>

              <textarea
                className="device-files-editor-area custom-scrollbar"
                value={editorContent}
                onChange={(e) => setEditorContent(e.target.value)}
              />

              <div style={{ display: "flex", gap: 8, justifyContent: "flex-end", marginTop: 4 }}>
                <Button
                  variant="primary"
                  size="sm"
                  icon={<Save size={13} />}
                  disabled={noDevice}
                  onClick={() => void handleSave()}
                >
                  保存修改
                </Button>
              </div>
            </div>
          ) : (
            <div style={{ textAlign: "center", color: "var(--text-tertiary)", fontSize: 12, padding: "20px 0" }}>
              选择文本或配置文件以进行即时预览与修改
            </div>
          )}
        </aside>
      </div>
    </div>
  );
};

export default DeviceFilesView;
