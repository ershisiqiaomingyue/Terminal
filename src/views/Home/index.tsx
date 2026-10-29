import React, { useCallback, useState } from "react";
import {
  Layers,
  LineChart,
  ScrollText,
  LayoutGrid,
  Terminal,
  Package,
  Smartphone,
  Archive,
  Hammer,
  Cpu,
  FileCode,
  Database,
  BarChart3,
  Bug,
  FolderSync,
  Camera,
  SlidersHorizontal,
  PanelTop,
  SquareCode,
  ExternalLink,
} from "lucide-react";
import { type SupportedToolKey } from "@/views/ToolWindows";
import { ContextMenu } from "@/components/ui/ContextMenu";
import { ToolIcon } from "@/components/ToolIcon/ToolIcon";

export interface HomeTool {
  id: string;
  title: string;
  subtitle: string;
  desc: string;
  tag: string;
  icon: React.ReactNode;
  iconClass: string;
  toolKey?: SupportedToolKey;
  status?: "ready" | "unavailable" | "planned";
}

export type HomeToolStatus = "ready" | "unavailable" | "planned";

export function homeToolStatus(tool: HomeTool): HomeToolStatus {
  return tool.status || (tool.toolKey ? "ready" : "planned");
}

export function homeToolStatusLabel(status: HomeToolStatus) {
  return status === "ready" ? "可用" : status === "unavailable" ? "不可用" : "待开发";
}

export function homeToolStatusVariant(status: HomeToolStatus): "success" | "danger" | "neutral" {
  return status === "ready" ? "success" : status === "unavailable" ? "danger" : "neutral";
}

interface HomeViewProps {
  onNavigateToTable?: () => void;
  onOpenTool?: (toolId: SupportedToolKey) => void;
  onOpenToolWindow?: (toolId: SupportedToolKey) => void;
}

// 原项目 Android-Toolkit 完整 17 项工具清单，同时作为全局快捷工具面板的数据源。
export const HOME_TOOLS: HomeTool[] = [
    {
      id: "code-viewer", title: "代码查看", subtitle: "本地 / SSH 目录树",
      desc: "按需展开本地工程或 SSH 目录树，查看源码和 Markdown，可在新标签或独立窗口打开。",
      tag: "已集成", icon: <SquareCode size={18} />, iconClass: "icon-indigo", toolKey: "code-viewer",
    },
    {
      id: "winscope",
      title: "WinScope",
      subtitle: "WindowManager & SurfaceFlinger",
      desc: "WindowManager, SurfaceFlinger 状态分析与可视追踪，查看窗口分层与渲染树。",
      tag: "已集成",
      icon: <Layers size={18} />,
      iconClass: "icon-indigo",
      status: "planned",
    },
    {
      id: "perfetto",
      title: "Perfetto",
      subtitle: "SysTrace 系统性能追踪",
      desc: "系统级 CPU、内存与调度性能追踪数据可视化，支持导入外部 trace 文件分析。",
      tag: "性能分析",
      icon: <LineChart size={18} />,
      iconClass: "icon-sky",
      toolKey: "perf",
    },
    {
      id: "logcat",
      title: "Logcat 日志",
      subtitle: "实时日志捕获与检索",
      desc: "实时捕获系统运行日志流，支持多 PID 绑定、Crash 异常过滤与关键字正则检索。",
      tag: "可用",
      icon: <ScrollText size={18} />,
      iconClass: "icon-amber",
      toolKey: "logcat",
    },
    {
      id: "layout-inspector",
      title: "Layout Inspector",
      subtitle: "UI Automator 层级分析",
      desc: "导出当前窗口 UI Automator 层级 XML，动态查看界面节点属性与布局 bounds。",
      tag: "可用",
      icon: <LayoutGrid size={18} />,
      iconClass: "icon-rose",
    },
    {
      id: "adb-terminal",
      title: "终端",
      subtitle: "本地 / WSL / SSH / ADB 多源终端",
      desc: "在一个界面里管理本机、WSL、SSH 与 ADB 终端会话，左侧切换、右侧查看主机状态与文件。",
      tag: "可用",
      icon: <Terminal size={18} />,
      iconClass: "icon-emerald",
      toolKey: "shell",
    },
    {
      id: "packages",
      title: "应用包管理器",
      subtitle: "应用生命周期控制",
      desc: "快速卸载、清除应用缓存、APK 提取安装、权限清单查看与后台进程强行停止。",
      tag: "可用",
      icon: <Package size={18} />,
      iconClass: "icon-red",
      toolKey: "apps",
    },
    {
      id: "adb",
      title: "ADB 设备",
      subtitle: "连接状态与 Server 管理",
      desc: "检测已连接 USB/WiFi 设备、查看详细连接状态并管理本机 ADB server 端口。",
      tag: "可用",
      icon: <Smartphone size={18} />,
      iconClass: "icon-sky",
      toolKey: "adb",
    },
    {
      id: "archive",
      title: "APK / JAR 包查看器",
      subtitle: "安装包解析与文件树",
      desc: "查看 APK 文件树结构、SHA-256 校验和、资源对比并安全解包分析清单。",
      tag: "可用",
      icon: <Archive size={18} />,
      iconClass: "icon-amber",
      toolKey: "archive",
    },
    {
      id: "aosp-build",
      title: "AOSP 构建",
      subtitle: "源码探测与模块编译",
      desc: "通过已配置的 SSH 或 WSL 执行 Android 源码探测、lunch 和模块增量构建。",
      tag: "可用",
      icon: <Hammer size={18} />,
      iconClass: "icon-slate",
      toolKey: "aosp",
    },
    {
      id: "simpleperf",
      title: "Native CPU 热点",
      subtitle: "Simpleperf 采样分析",
      desc: "使用设备 Simpleperf 采集 perf.data，并按符号表查看 C/C++ 热点函数 CPU 占比。",
      tag: "可用",
      icon: <Cpu size={18} />,
      iconClass: "icon-indigo",
      toolKey: "simpleperf",
    },
    {
      id: "method-profiler",
      title: "Java 方法采样",
      subtitle: "Trace 采样与调用栈",
      desc: "采集 Java / Kotlin 方法运行时 Trace，并无缝导出给 Android Studio 进行排查。",
      tag: "可用",
      icon: <FileCode size={18} />,
      iconClass: "icon-emerald",
      toolKey: "method-profiler",
    },
    {
      id: "heap-dump",
      title: "Java 堆转储",
      subtitle: "HPROF 内存快照导出",
      desc: "导出可调试应用 HPROF 内存快照，并自动转换为 Android Studio 可读格式分析泄漏。",
      tag: "可用",
      icon: <Database size={18} />,
      iconClass: "icon-purple",
      toolKey: "heap-dump",
    },
    {
      id: "benchmark",
      title: "基准与帧卡顿",
      subtitle: "AndroidX Macrobenchmark",
      desc: "导入 AndroidX Macrobenchmark 结果，查看应用冷启动耗时、帧耗时与 Jank 卡顿。",
      tag: "可用",
      icon: <BarChart3 size={18} />,
      iconClass: "icon-pink",
      toolKey: "benchmark",
    },
    {
      id: "native-symbolizer",
      title: "Native 符号化",
      subtitle: "Tombstone 崩溃匹配",
      desc: "匹配系统 tombstone 与本地未剥离符号目录，快速定位 Native 核心崩溃栈代码行。",
      tag: "可用",
      icon: <Bug size={18} />,
      iconClass: "icon-slate",
      toolKey: "native-symbolizer",
    },
    {
      id: "device-files",
      title: "设备文件与互传",
      subtitle: "存储浏览与文件传输",
      desc: "浏览设备内部存储、批量拉取推送文件，极速双向传输与在线预览编辑。",
      tag: "可用",
      icon: <FolderSync size={18} />,
      iconClass: "icon-sky",
      toolKey: "files",
    },
    {
      id: "device-capture",
      title: "设备截图与录屏",
      subtitle: "快速捕获屏幕内容",
      desc: "一键捕获当前设备屏幕画面、导出超清 PNG 截图或高质量 MP4 屏幕录像。",
      tag: "可用",
      icon: <Camera size={18} />,
      iconClass: "icon-amber",
      toolKey: "capture",
    },
    {
      id: "device-control",
      title: "设备快捷控制",
      subtitle: "系统调试与硬件控制",
      desc: "控制屏幕方向、硬件导航键、音量、Logcat 级别与设备 Fastboot/Recovery 启动模式。",
      tag: "可用",
      icon: <SlidersHorizontal size={18} />,
      iconClass: "icon-purple",
      toolKey: "control",
    },
];

export const HOME_TOOL_GROUPS = [
  // 终端（adb-terminal）不在这里：它常驻左上导航栏，右键还能在新窗口打开，
  // 主页再列一项只是重复入口。HOME_TOOLS 仍保留该项，顶栏搜索与独立窗口选单照旧找得到。
  { title: "设备与应用", ids: ["adb", "packages", "device-files", "device-capture", "device-control"] },
  { title: "日志与性能", ids: ["logcat", "perfetto", "simpleperf", "method-profiler", "heap-dump", "benchmark"] },
  { title: "构建与分析", ids: ["code-viewer", "archive", "aosp-build", "native-symbolizer", "winscope", "layout-inspector"] },
].map((group) => ({ ...group, tools: group.ids.map((id) => HOME_TOOLS.find((tool) => tool.id === id)!) }));

export const HomeView: React.FC<HomeViewProps> = ({ onOpenTool, onOpenToolWindow }) => {
  const [menu, setMenu] = useState<{ tool: HomeTool; x: number; y: number; trigger: HTMLButtonElement } | null>(null);
  const closeMenu = useCallback(() => setMenu(null), []);
  return (
  <div className="home-view">
    <h1 className="home-title">工具</h1>
    {HOME_TOOL_GROUPS.map((group) => (
      <section className="home-tool-group" key={group.title} aria-label={group.title}>
        <h2>{group.title}</h2>
        <div className="home-tool-list">
          {group.tools.map((tool) => {
            const status = homeToolStatus(tool);
            return (
              <button
                type="button"
                key={tool.id}
                className={`home-tool-row is-${status}`}
                disabled={status !== "ready"}
                title={`${status === "ready" ? "已实现 · 可用" : status === "unavailable" ? "不可用" : "禁用 · 待开发"}：${tool.desc}`}
                onClick={() => tool.toolKey && onOpenTool?.(tool.toolKey)}
                onContextMenu={(event) => {
                  if (!tool.toolKey || status !== "ready") return;
                  event.preventDefault();
                  const rect = event.currentTarget.getBoundingClientRect();
                  setMenu({ tool, x: event.clientX || rect.left + 16, y: event.clientY || rect.bottom, trigger: event.currentTarget });
                }}
              >
                <ToolIcon className="home-tool-icon">{tool.icon}</ToolIcon>
                <span className="home-tool-copy">
                  <span className="home-tool-title">{tool.title}</span>
                  <span className="home-tool-sub">{tool.subtitle}</span>
                </span>
                <span className={`home-tool-status is-${status}`} role="img" aria-label={status === "planned" ? "禁用 · 待开发" : homeToolStatusLabel(status)} title={status === "ready" ? "已实现 · 可用" : status === "unavailable" ? "不可用" : "禁用 · 待开发"} />
              </button>
            );
          })}
        </div>
      </section>
    ))}
    {menu && <ContextMenu x={menu.x} y={menu.y} label={`${menu.tool.title}操作`} trigger={menu.trigger} onClose={closeMenu} items={[
      { label: "在当前窗口中打开", icon: <PanelTop />, onSelect: () => onOpenTool?.(menu.tool.toolKey!) },
      { label: "在新窗口中打开", icon: <ExternalLink />, disabled: !onOpenToolWindow, onSelect: () => onOpenToolWindow?.(menu.tool.toolKey!) },
    ]} />}
  </div>
  );
};

export default HomeView;
