import { NoticePermissionContent, NoticeQuestionContent, NoticeQueueRow, NoticeBackgroundTask } from "@/components/ui/NoticeContent";
import { SegmentedControl } from "@/components/ui/SegmentedControl";
import React, { useEffect, useState } from "react";
import {
  Search,
  ClipboardList,
  LoaderCircle,
  Sparkles,
  Download,
  Trash2,
  RefreshCw,
  Cpu,
  HardDrive,
  ShieldCheck,
  KeyRound,
  Wrench,
  Smartphone,
  Package,
  Battery,
  CheckCircle2,
  AlertTriangle,
  Copy,
  FolderOpen,
  Info,
  Settings2,
  Terminal,
  ListPlus,
  MoreHorizontal,
  MessageCircleQuestion,
  ArrowUp,
  Pencil,
} from "lucide-react";
import {
  Card,
  CardHeader,
  CardBody,
  CardFooter,
  Button,
  Select,
  Switch,
  Badge,
  Table,
  Divider,
  Input,
  TaskStepList,
  ToastCard,
  useToast,
  ToastProvider,
  type ToastTone,
  NoticePanel,
  Attachment,
  attachmentKindOf,
  ProgressRing,
  AccessModeMenu,
  type AccessMode,
  CompactionProgress,
  UsageMetrics,
  FilePreviewPanel,
  FileDiffView,
  FileChangeCard,
  FileChangeNotice,
} from "@/components/ui";
import { ToolActivityStatus } from "@/views/Chat/ToolActivityStatus";
import { AgentModelPicker, type AgentModelSelection } from "@/components/AgentModelPicker/AgentModelPicker";
import type { AgentProvider } from "@/platform/api";
import type { ReasoningEffort } from "@/platform/modelCapabilities";
import { readComponentDebugTargets, subscribeComponentDebugTargets } from "@/platform/componentDebugPreferences";

/** Section shell so every block reads the same way. */
const Section: React.FC<{
  title: string;
  desc?: string;
  aside?: React.ReactNode;
  children: React.ReactNode;
}> = ({ title, desc, aside, children }) => {
  // The section id is the leading number ("01", "02", ...) so the sidebar can
  // link to it directly instead of searching for heading text.
  const anchor = title.slice(0, 2);
  return (
  <Card dividers data-section={anchor} id={`component-section-${anchor}`} style={{ scrollMarginTop: 12 }}>
    <CardHeader>
      <div>
        <h3 className="card-title">{title}</h3>
        {desc && <div className="card-subtitle">{desc}</div>}
      </div>
      {aside}
    </CardHeader>
    <CardBody style={{ display: "flex", flexDirection: "column", gap: 20 }}>{children}</CardBody>
  </Card>
  );
};

/** Label above a demo row. */
const Row: React.FC<{ label: string; hint?: string; children: React.ReactNode }> = ({
  label,
  hint,
  children,
}) => (
  <div>
    <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 10, color: "var(--text-secondary)" }}>
      {label}
      {hint && (
        <span style={{ marginLeft: 8, fontWeight: 400, color: "var(--text-tertiary)" }}>{hint}</span>
      )}
    </div>
    {children}
  </div>
);

/** A single property line used inside the demo cards. */
const Prop: React.FC<{ icon: React.ReactNode; label: string; value: string }> = ({
  icon,
  label,
  value,
}) => (
  <div className="card-property-row">
    <span className="card-property-label">
      {icon}
      {label}
    </span>
    <span className="card-property-value">{value}</span>
  </div>
);

/** Demo providers that mirror the real AgentProvider shape. */
const DEMO_PROVIDERS: AgentProvider[] = [
  {
    id: "deepseek",
    name: "DeepSeek",
    kind: "openai_responses",
    brand: "deepseek",
    baseUrl: "https://api.deepseek.com",
    model: "deepseek-v4-flash",
    models: [
      {
        id: "deepseek-v4-flash",
        name: "DeepSeek V4 Flash",
        contextWindow: 131072,
        supportsReasoning: true,
        reasoningEfforts: ["medium", "high"] as ReasoningEffort[],
        billing: { multiplier: 1, badges: ["限免"], free: true },
      },
      {
        id: "deepseek-v4-pro",
        name: "DeepSeek V4 Pro",
        contextWindow: 262144,
        supportsReasoning: true,
        reasoningEfforts: ["medium", "high"] as ReasoningEffort[],
        billing: { multiplier: 2.5, badges: [], free: false, rateUnknown: false },
      },
    ],
  },
  {
    id: "anthropic",
    name: "Anthropic",
    kind: "anthropic",
    brand: "claude",
    baseUrl: "https://api.anthropic.com",
    model: "claude-sonnet",
    models: [
      {
        id: "claude-sonnet",
        name: "Claude Sonnet 4.6",
        contextWindow: 200000,
        supportsReasoning: true,
        reasoningEfforts: ["low", "medium", "high"] as ReasoningEffort[],
        billing: { multiplier: 3, badges: [], free: false, rateUnknown: false },
      },
      {
        id: "claude-haiku",
        name: "Claude Haiku 4.5",
        contextWindow: 200000,
        supportsReasoning: false,
        billing: { multiplier: 0.8, badges: [], free: false, rateUnknown: false },
      },
    ],
  },
];

const ComponentsGallery: React.FC = () => {
  const [componentDebugEnabled, setComponentDebugEnabled] = useState(readComponentDebugTargets().components);
  useEffect(() => subscribeComponentDebugTargets((targets) => setComponentDebugEnabled(targets.components)), []);
  const [switch1, setSwitch1] = useState(true);
  const [switch2, setSwitch2] = useState(false);
  const [inputValue, setInputValue] = useState("Pixel_8_Pro_Debug_v2.apk");
  const [seg, setSeg] = useState<"all" | "third" | "sys">("all");
  const [modelSelection, setModelSelection] = useState<AgentModelSelection>({
    providerId: "deepseek",
    modelId: "deepseek-v4-flash",
  });
  const [inferenceLevel, setInferenceLevel] = useState<ReasoningEffort>("medium" as ReasoningEffort);
  const toast = useToast();
  const [galleryTone, setGalleryTone] = useState<ToastTone>("success");
  const [answerDraft, setAnswerDraft] = useState("");
  const [accessMode, setAccessMode] = useState<AccessMode>("ask");

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 28, maxWidth: 1200, margin: "0 auto" }}>
      {/* ── 概览 ─────────────────────────────────────────────────────────── */}
      <div className="home-section-header">
        <div>
          <h2 style={{ fontSize: 18, fontWeight: 700, color: "var(--text-primary)" }}>
            Android-Toolkit 组件库
          </h2>
          <p style={{ fontSize: 13, color: "var(--text-tertiary)", marginTop: 4 }}>
            按基础原子、容器、数据展示、选择器、状态反馈归类；全部取自设计令牌，亮暗双模一致
          </p>
        </div>
        <div style={{ display: "flex", gap: 8 }}>
          <Badge variant="primary">React 18 + TSX</Badge>
          {componentDebugEnabled && <Badge variant="success">组件调试已启用</Badge>}
          <Badge variant="success" pulse>
            令牌驱动
          </Badge>
        </div>
      </div>

      {/* ── 01 卡片配色 ──────────────────────────────────────────────────── */}
      <Section
        title="01 · Card 四种配色"
        desc="default 中性玻璃 / accent 品牌靛青 / success 通过绿 / warning 警示琥珀；几何不变，只换表面与描边"
        aside={<Badge variant="primary">配色体系</Badge>}
      >
        <Row
        label="四级中性底色"
        hint="同一套卡片材质，只换表面灰度：做页面布局时按内容层级选一级，分割线自动跟着表面深浅调整"
      >
        <div style={{ display: "grid", gridTemplateColumns: "repeat(2, minmax(0,1fr))", gap: 16 }}>
          <Card tone="surface" dividers>
            <CardHeader>
              <div>
                <h4 className="card-title">surface · 纯白</h4>
                <div className="card-subtitle">最前层，主内容面板</div>
              </div>
              <Badge variant="neutral">1</Badge>
            </CardHeader>
            <div>
              <Prop icon={<HardDrive size={14} />} label="总存储空间" value="256 GB" />
              <Prop icon={<Cpu size={14} />} label="CPU 架构" value="arm64-v8a" />
            </div>
          </Card>

          <Card tone="muted" dividers>
            <CardHeader>
              <div>
                <h4 className="card-title">muted · 浅灰</h4>
                <div className="card-subtitle">次层，并列信息分组</div>
              </div>
              <Badge variant="neutral">2</Badge>
            </CardHeader>
            <div>
              <Prop icon={<Battery size={14} />} label="电池状态" value="88%" />
              <Prop icon={<Smartphone size={14} />} label="设备型号" value="Pixel 8 Pro" />
            </div>
          </Card>

          <Card tone="subtle" dividers>
            <CardHeader>
              <div>
                <h4 className="card-title">subtle · 中灰</h4>
                <div className="card-subtitle">再次层，辅助与统计信息</div>
              </div>
              <Badge variant="neutral">3</Badge>
            </CardHeader>
            <div>
              <Prop icon={<Package size={14} />} label="已装应用" value="218 个" />
              <Prop icon={<ShieldCheck size={14} />} label="SELinux" value="Enforcing" />
            </div>
          </Card>

          <Card tone="deep" dividers>
            <CardHeader>
              <div>
                <h4 className="card-title">deep · 深灰</h4>
                <div className="card-subtitle">最深，弱化背景块与占位区</div>
              </div>
              <Badge variant="neutral">4</Badge>
            </CardHeader>
            <div>
              <Prop icon={<KeyRound size={14} />} label="Root 权限" value="未授予" />
              <Prop icon={<Wrench size={14} />} label="分区挂载" value="只读" />
            </div>
          </Card>
        </div>
      </Row>

      <Divider dense />
        <Row label="带分割线的属性卡片" hint="图 2 结构：图标 + 标签左，数值右，行间极虚分割线">
          <Card dividers style={{ maxWidth: 460 }}>
            <CardHeader>
              <div>
                <h4 className="card-title">设备概览</h4>
                <div className="card-subtitle">属性行使用 card-property-row</div>
              </div>
            </CardHeader>
            <div>
              <Prop icon={<HardDrive size={14} />} label="总存储空间" value="256 GB" />
              <Prop icon={<Battery size={14} />} label="电池状态" value="88%" />
              <Prop icon={<ShieldCheck size={14} />} label="Root 访问权限" value="未授予" />
              <Prop icon={<Smartphone size={14} />} label="设备型号" value="Pixel 8 Pro" />
            </div>
          </Card>
        </Row>
      </Section>

      {/* ── 02 按钮 ──────────────────────────────────────────────────────── */}
      <Section
        title="02 · Button 按钮"
        desc="primary 强调 / secondary 底座 / danger 危险 / subtle 幽灵；图标按钮保持扁平以维持工具条密度"
      >
        <Row label="变体矩阵">
          <div style={{ display: "flex", flexWrap: "wrap", gap: 10, alignItems: "center" }}>
            <Button variant="primary" icon={<Sparkles size={13} />}>
              Primary 强调
            </Button>
            <Button variant="secondary">Secondary 底座</Button>
            <Button variant="danger" icon={<Trash2 size={13} />}>
              Danger 危险
            </Button>
            <Button variant="subtle">Subtle 幽灵</Button>
            <Button variant="primary" loading>
              加载中
            </Button>
            <Button variant="secondary" icon={<KeyRound size={13} />}>
              请求 Root
            </Button>
            <Button variant="secondary" icon={<Wrench size={13} />}>
              重新挂载
            </Button>
          </div>
        </Row>

        <Divider dense />

        <Row label="尺寸" hint="xs / sm / md / lg">
          <div style={{ display: "flex", flexWrap: "wrap", gap: 10, alignItems: "center" }}>
            <Button variant="secondary" size="xs">
              xs
            </Button>
            <Button variant="secondary" size="sm">
              sm
            </Button>
            <Button variant="secondary" size="md">
              md
            </Button>
            <Button variant="secondary" size="lg">
              lg
            </Button>
          </div>
        </Row>

        <Divider dense />

        <Row label="图标按钮 · 尺寸" hint="只传 icon、不传 children 时自动成为方形按钮（aspect-ratio: 1）">
          <div style={{ display: "flex", flexWrap: "wrap", gap: 10, alignItems: "center" }}>
            <Button variant="subtle" size="xs" icon={<Copy size={13} />} title="复制" aria-label="复制" />
            <Button variant="subtle" size="sm" icon={<Copy size={14} />} title="复制" aria-label="复制" />
            <Button variant="subtle" size="md" icon={<Copy size={15} />} title="复制" aria-label="复制" />
            <Button variant="subtle" size="lg" icon={<Copy size={16} />} title="复制" aria-label="复制" />
          </div>
        </Row>

        <Row label="图标按钮 · 语义动作" hint="subtle 保持透明扁平，避免工具条被磨砂方块填满；danger-hover 悬停才转红">
          <div style={{ display: "flex", flexWrap: "wrap", gap: 10, alignItems: "center" }}>
            <Button variant="subtle" size="sm" icon={<RefreshCw size={14} />} title="刷新" aria-label="刷新" />
            <Button variant="subtle" size="sm" icon={<Settings2 size={14} />} title="设置" aria-label="设置" />
            <Button variant="subtle" size="sm" icon={<FolderOpen size={14} />} title="打开目录" aria-label="打开目录" />
            <Button variant="subtle" size="sm" icon={<MoreHorizontal size={14} />} title="更多" aria-label="更多" />
            <Button variant="subtle" size="sm" className="btn-danger-hover" icon={<Trash2 size={14} />} title="删除" aria-label="删除" />
          </div>
        </Row>

        <Row label="图标按钮 · 带文字对照" hint="同样传 icon，但补上 children 就变成常规胶囊按钮">
          <div style={{ display: "flex", flexWrap: "wrap", gap: 10, alignItems: "center" }}>
            <Button variant="subtle" size="sm" icon={<Copy size={14} />}>复制路径</Button>
            <Button variant="subtle" size="sm" icon={<RefreshCw size={14} />} loading>刷新中</Button>
          </div>
        </Row>
      </Section>

      {/* ── 03 表单与开关 ──────────────────────────────────────────────────── */}
      <Section
        title="03 · 表单原子"
        desc="Input 内凹槽位与清除、复合输入槽、Switch 开关、SegmentedControl 分段控制：同一批表单控件放在一起对照"
      >
        <Row label="Input · 带前缀与清除" hint="内凹槽位、前缀图标与一键清除">
            <Input
              size="md"
              prefixIcon={<Search size={14} />}
              value={inputValue}
              onChange={(e) => setInputValue(e.target.value)}
              clearable
              onClear={() => setInputValue("")}
              placeholder="搜索应用包名或执行指令..."
            />
        </Row>

        <Row label="Switch · 开关" hint="开启 / 关闭 / 禁用三态">
            <div style={{ display: "flex", alignItems: "center", gap: 20, flexWrap: "wrap" }}>
              <Switch checked={switch1} onChange={setSwitch1} label="开启状态" />
              <Switch checked={switch2} onChange={setSwitch2} label="关闭状态" />
              <Switch disabled checked label="禁用激活" />
            </div>
        </Row>


        <Row label="SegmentedControl · 分段控制" hint="选项数量固定时替代下拉，选中态跟选中项走">
            <SegmentedControl
              value={seg}
              onChange={setSeg}
              label="进程类型"
              options={[
                { value: "all", label: "全部 (218)" },
                { value: "third", label: "第三方 (42)" },
                { value: "sys", label: "系统 (176)" },
              ]}
            />
        </Row>
      </Section>

      {/* ── 04 选择器 ────────────────────────────────────────────────────── */}
      <Section
        title="04 · Select 选择器"
        desc="通用下拉：可搜索、分组、选项右侧操作槽、选项内嵌第二层弹窗"
      >
        <div style={{ display: "grid", gridTemplateColumns: "repeat(2, minmax(0,1fr))", gap: 20 }}>
          <Row label="基础下拉">
            <Select
              size="md"
              defaultValue="120hz"
              options={[
                { label: "自动适配刷新率 (Dynamic 120Hz)", value: "120hz" },
                { label: "锁定 90Hz 高平滑度", value: "90hz" },
                { label: "锁定 60Hz 节能", value: "60hz" },
              ]}
            />
          </Row>

          <Row label="分组 + 可搜索">
            <Select
              size="md"
              searchable
              searchPlaceholder="搜索设备..."
              defaultValue="pixel"
              options={[
                { label: "Pixel 8 Pro", value: "pixel", group: { id: "local", label: "本机设备" } },
                { label: "Pixel 7a", value: "pixel7a", group: { id: "local", label: "本机设备" } },
                { label: "Xiaomi 14", value: "mi14", group: { id: "remote", label: "远程设备" } },
              ]}
            />
          </Row>
        </div>
      </Section>

      {/* 模型选择器（对话页同款）──────────────────────────────────── */}
      {/* The picker opens its menu upward, so this card must not clip it. */}
      <Card dividers style={{ overflow: "visible" }}>
        <CardHeader>
          <div>
            <h3 className="card-title">Model 选择器 · AgentModelPicker</h3>
            <div className="card-subtitle">
              与对话页完全同源的实现：供应商行 / 模型行 / 推理强度轨道三级折叠；菜单向上弹出
            </div>
          </div>
          <Badge variant="primary">对话页同款</Badge>
        </CardHeader>
        <CardBody style={{ display: "flex", flexDirection: "column", gap: 24 }}>
          <Row
            label="完整交互"
            hint="点击触发条展开：供应商 → 模型（可搜索、带倍率）→ 推理强度（可拖拽轨道）"
          >
            <div
              style={{
                padding: 24,
                borderRadius: 12,
                background: "var(--surface-inset-bg)",
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                gap: 16,
                flexWrap: "wrap",
              }}
            >
              <AgentModelPicker
                providers={DEMO_PROVIDERS}
                selection={modelSelection}
                inferenceLevel={inferenceLevel}
                onChange={setModelSelection}
                onInferenceLevelChange={setInferenceLevel}
                onOpenSettings={() => undefined}
              />
              <span style={{ fontSize: 12, color: "var(--text-tertiary)" }}>
                当前：{modelSelection.providerId} / {modelSelection.modelId} · 推理 {inferenceLevel}
              </span>
            </div>
          </Row>

          <Divider dense />

          <Row label="禁用态" hint="供应商与模型数据同样来自上面的演示数据">
            <div
              style={{
                padding: 24,
                borderRadius: 12,
                background: "var(--surface-inset-bg)",
                display: "flex",
                justifyContent: "center",
              }}
            >
              <AgentModelPicker
                providers={DEMO_PROVIDERS}
                selection={modelSelection}
                disabled
                onChange={() => undefined}
                onOpenSettings={() => undefined}
              />
            </div>
          </Row>
        </CardBody>
      </Card>

      {/* ── 05 数据展示 ──────────────────────────────────────────────────── */}
      <Section
        title="05 · Table 数据表格"
        desc="极虚分割线表头与行、等宽数值、行内操作"
        aside={
          <div style={{ display: "flex", gap: 8 }}>
            <Button variant="secondary" size="sm" icon={<Download size={13} />}>
              导出 CSV
            </Button>
            <Button variant="primary" size="sm" icon={<RefreshCw size={13} />}>
              刷新
            </Button>
          </div>
        }
      >
        <Table>
          <thead>
            <tr>
              <th>应用名称</th>
              <th>Package</th>
              <th>PID</th>
              <th>CPU</th>
              <th>PSS</th>
              <th>状态</th>
              <th style={{ textAlign: "right" }}>操作</th>
            </tr>
          </thead>
          <tbody>
            <tr>
              <td style={{ fontWeight: 600, color: "var(--text-primary)" }}>微信</td>
              <td>
                <span className="table-cell-mono">com.tencent.mm</span>
              </td>
              <td>
                <span className="table-cell-mono">14208</span>
              </td>
              <td>
                <span className="table-cell-mono">2.8%</span>
              </td>
              <td>
                <span className="table-cell-mono">342 MB</span>
              </td>
              <td>
                <Badge variant="success" pulse>
                  前台活跃
                </Badge>
              </td>
              <td style={{ textAlign: "right" }}>
                <Button variant="subtle" size="xs">
                  导出日志
                </Button>
              </td>
            </tr>
            <tr>
              <td style={{ fontWeight: 600, color: "var(--text-primary)" }}>抖音</td>
              <td>
                <span className="table-cell-mono">com.ss.android.ugc.aweme</span>
              </td>
              <td>
                <span className="table-cell-mono">18933</span>
              </td>
              <td>
                <span className="table-cell-mono">14.5%</span>
              </td>
              <td>
                <span className="table-cell-mono">680 MB</span>
              </td>
              <td>
                <Badge variant="warning" pulse>
                  高负载
                </Badge>
              </td>
              <td style={{ textAlign: "right" }}>
                <Button variant="danger" size="xs">
                  强行停止
                </Button>
              </td>
            </tr>
          </tbody>
        </Table>
        <CardFooter style={{ justifyContent: "space-between", padding: "12px 0 0", borderTop: 0 }}>
          <span style={{ fontSize: 12, color: "var(--text-tertiary)" }}>显示第 1 至 2 项</span>
          <div style={{ display: "flex", gap: 6 }}>
            <Button variant="secondary" size="xs" disabled>
              上一页
            </Button>
            <Button variant="secondary" size="xs">
              下一页
            </Button>
          </div>
        </CardFooter>
      </Section>

      {/* ── 06 状态反馈 ──────────────────────────────────────────────────── */}
      <Section
        title="06 · Badge 状态与呼吸灯"
        desc="脉冲为真实动效：扩散光晕 + 透明度与缩放呼吸；prefers-reduced-motion 下自动静止"
      >
        <Row label="状态胶囊 + 呼吸指示">
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
            <Badge variant="primary">Primary</Badge>
            <Badge variant="success" pulse>
              在线 (Pulse)
            </Badge>
            <Badge variant="warning" pulse>
              等待授权
            </Badge>
            <Badge variant="danger">离线断连</Badge>
            <Badge variant="neutral">系统进程</Badge>
          </div>
        </Row>

        <Divider dense />

        <Row label="独立呼吸点" hint="用于列表、设备状态等轻量场景">
          <div style={{ display: "flex", alignItems: "center", gap: 28, flexWrap: "wrap" }}>
            <span style={{ display: "inline-flex", alignItems: "center", gap: 8, color: "var(--success-text)" }}>
              <span className="pulse-dot" />
              <span style={{ fontSize: 12, color: "var(--text-secondary)" }}>设备在线</span>
            </span>
            <span style={{ display: "inline-flex", alignItems: "center", gap: 8, color: "var(--warning-text)" }}>
              <span className="pulse-dot" />
              <span style={{ fontSize: 12, color: "var(--text-secondary)" }}>等待授权</span>
            </span>
            <span style={{ display: "inline-flex", alignItems: "center", gap: 8, color: "var(--accent-primary)" }}>
              <span className="pulse-dot" />
              <span style={{ fontSize: 12, color: "var(--text-secondary)" }}>任务运行中</span>
            </span>
          </div>
        </Row>
      </Section>
      {/* ── 07 任务步骤 ──────────────────────────────────────────────────── */}
      <Section
        title="07 · TaskStepList 任务步骤"
        desc="AI 回复里的指令执行清单：完成 = 实心勾，执行中 = 转圈，待处理 = 空心圆环；与对话页任务时间线同源"
        aside={<Badge variant="primary">showcase 同源</Badge>}
      >
        <div style={{ display: "grid", gridTemplateColumns: "repeat(2, minmax(0,1fr))", gap: 20 }}>
          <Row label="常规密度" hint="task-step-list 默认 10px 间距">
            <TaskStepList
              steps={[
                { status: "done", content: <>Install <b>Slack</b> app</> },
                { status: "done", content: <>Create channel <b>design-team</b></> },
                { status: "running", content: <>Invite users to channel</>, chip: "PW · Pat" },
                { status: "pending", content: "Verify webhook payloads", chip: "Auto Test" },
              ]}
            />
          </Row>

          <Row label="紧凑密度" hint="compact：对话页里每个 item 竖直方向压缩后的形态">
            <TaskStepList
              compact
              steps={[
                { status: "done", content: "拉取 AOSP 分支" },
                { status: "done", content: "设置 lunch 目标", chip: "aosp_husky-userdebug" },
                { status: "running", content: "编译 SystemUI 模块" },
                { status: "pending", content: "推送并验证产物" },
              ]}
            />
          </Row>
        </div>
      </Section>

      {/* ── 08 Toast ─────────────────────────────────────────────────────── */}
      <Section
        title="08 · Toast 轻提示"
        desc="全应用共用一套右下角堆叠：色调只由图标承担，表面保持中性，所以连续弹多条不会变成一排彩色块；错误停留 4.8s，确认类 3s"
        aside={<Badge variant="primary">ToastProvider</Badge>}
      >
        <Row label="静态样片" hint="ToastCard 是纯展示版本，可直接放进页面预览">
          <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
            <ToastCard tone="success" message="技能已加载，共 14 项可调用。" />
            <ToastCard tone="info" message="正在读取技能说明…" />
            <ToastCard tone="error" message="连接失败：目标设备未授权 ADB 调试。" actionLabel="重试" />
          </div>
        </Row>

        <Divider dense />

        <Row label="交互触发" hint="点击后从右下角升起，可用右上角关闭，也可挂一个动作按钮">
          <div style={{ display: "flex", flexWrap: "wrap", gap: 10, alignItems: "center" }}>
            <Button variant="secondary" size="sm" onClick={() => toast.success("技能已启用。")}>success</Button>
            <Button variant="secondary" size="sm" onClick={() => toast.info("正在同步远程目录…")}>info</Button>
            <Button variant="secondary" size="sm" onClick={() => toast.error("构建失败：找不到 lunch 目标。")}>error</Button>
            <Button
              variant="secondary"
              size="sm"
              onClick={() => toast.push({ message: "已删除 3 条会话。", tone: "success", action: { label: "撤销", onClick: () => toast.info("已撤销。") } })}
            >
              带撤销动作
            </Button>
          </div>
        </Row>
      </Section>

      <Section
        title="09 · NoticePanel 通知面板"
        desc="输入框上方那组折叠行，产品里共有六类：权限申请 / AI 询问 / 任务编排 / 后台任务 / 追加消息 / 文件改动。整行可点，动作区独立不触发折叠；上下文压缩不在其中，它是独立的进度条"
        aside={<Badge variant="primary">6 类真实现</Badge>}
      >
        <Row label="权限申请 · 展开态" hint="等宽命令块 + 允许一次 / 本会话允许 / 拒绝，拒绝用危险色">
          <div style={{ display: "flex", flexDirection: "column", gap: 8, width: "100%", maxWidth: "min(720px, 100%)" }}>
            <NoticePanel
              label="权限申请"
              icon={<ShieldCheck size={15} />}
              summary="需要批准 · android_adb_action"
              tone="attention"
              defaultExpanded
            >
              <NoticePermissionContent command="adb shell pm uninstall com.example.debug" />
            </NoticePanel>
          </div>
        </Row>

        <Divider dense />

        <Row label="AI 询问 · 展开态" hint="tone=warning：琥珀左边线；带建议答案按钮">
          <div style={{ display: "flex", flexDirection: "column", gap: 8, width: "100%", maxWidth: "min(720px, 100%)" }}>
            <NoticePanel
              label="AI 询问"
              icon={<MessageCircleQuestion size={15} />}
              summary="需要回答 · 要把产物推送到哪台设备？"
              tone="warning"
              defaultExpanded
            >
              <NoticeQuestionContent options={["Pixel 8 Pro", "Redmi K60", "模拟器"]} value={answerDraft} onChange={setAnswerDraft} />
            </NoticePanel>
          </div>
        </Row>

        <Divider dense />

        <Row label="任务编排 · 点击展开" hint="右侧是耗时·进度 pill；点击展开；展开后的步骤列表用紧凑型（compact，gap 2px）">
          <div style={{ display: "flex", flexDirection: "column", gap: 8, width: "100%", maxWidth: "min(720px, 100%)" }}>
            <NoticePanel
              label="任务编排"
              icon={<ClipboardList size={15} />}
              summary="把 SystemUI 的改动编译并推送到 Pixel 8 Pro"
              meta={<span className="composer-task-progress-pill">1m 24s · 3/5</span>}
            >
              <TaskStepList
                compact
                steps={[
                  { status: "done", content: "拉取 AOSP 分支" },
                  { status: "done", content: "编译 SystemUI 模块" },
                  { status: "running", content: "推送并验证产物" },
                ]}
              />
            </NoticePanel>
          </div>
        </Row>

        <Divider dense />

        <Row label="后台任务 · 紧凑展开" hint="摘要带「正在/需检查/已完成」前缀，右侧 count 是任务数；展开后每个任务是一块独立内嵌面板，任务标题行本身就是折叠开关（整行可点，用时与停止按钮独立不触发折叠），收起只剩标题行；任务详情用组件库 Card 的 inset 形态，不直接裸露在通知背景上">
          <div style={{ display: "flex", flexDirection: "column", gap: 8, width: "100%", maxWidth: "min(720px, 100%)" }}>
            <NoticePanel
              label="后台任务"
              icon={<LoaderCircle size={15} />}
              summary="正在编译 AOSP · 2 项"
              tone="success"
              badge={<ToolActivityStatus state="running" />}
            >
              <NoticeBackgroundTask title="编译 SystemUI" running log={"> Building SystemUI...\n> [ 42%] Compiling resources"}>
                <details>
                  <summary>任务详情 · 编译 SystemUI 模块</summary>
                  <div>后台运行中 · 已执行 1分24秒</div>
                  <pre className="ui-notice-code" aria-label="后台任务完整命令" tabIndex={0}><code>./gradlew :SystemUI:assembleDebug</code></pre>
                  <small>任务 shell-demo1 · 保留最近日志（最多 16 MB）</small>
                </details>
              </NoticeBackgroundTask>
              <NoticeBackgroundTask title="打包 MFS_Tester.exe" defaultExpanded={false} log={"> 11426 INFO: Building EXE from EXE-00.toc\n> 10021 INFO: Build complete!"}>
                <details>
                  <summary>任务详情 · 执行任务并监测输出</summary>
                  <div>已完成 · 执行了 19秒 · 退出码 0</div>
                  <pre className="ui-notice-code" aria-label="后台任务完整命令" tabIndex={0}><code>pyinstaller --noconfirm MFS_Tester.spec</code></pre>
                  <small>任务 shell-demo2 · 保留最近日志（最多 16 MB）</small>
                </details>
              </NoticeBackgroundTask>
            </NoticePanel>
          </div>
        </Row>

        <Divider dense />

        <Row label="追加消息 · inline" hint="单条与多条共用同一套行：单条只有一行（呼吸灯 → 一条追加消息等待中：正文 → 右侧三个按钮），没有汇总头；多条抬头是「N 条追加消息等待中」+ 展开箭头，展开后每行带 01/02 行号与拖拽柄，删到只剩一条会自动退回单条形态">
          <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
            <NoticePanel label="" summary="" inline>
              <div className="ui-notice-queue-list is-single">
                <NoticeQueueRow queueId="demo-single" content="顺便把 lint 也跑一遍" single />
              </div>
            </NoticePanel>
            <NoticePanel
              label=""
              icon={<ListPlus size={15} />}
              summary=""
              summaryContent={<span className="ui-notice-queue-single-label">2 条追加消息等待中</span>}
              collapsible
              defaultExpanded
              inline
            >
              <div className="ui-notice-queue-list">
                {["顺便把 lint 也跑一遍", "然后把改动提交到 feature 分支"].map((content, index) => <NoticeQueueRow key={content} queueId={`demo-${index}`} content={content} index={index} />)}
              </div>
            </NoticePanel>
          </div>
        </Row>

        <Divider dense />

        <Row label="文件改动 · 占位展开 + +N −N" hint="只在 AI 回复过程中出现；点击展开、再次点击或 Escape 收起（点击外侧不收起），展开后药丸变成卡片、列表在卡片里向下铺开，卡片底色不透明，圆角 14px">
          <div style={{ display: "flex", flexDirection: "column", gap: 8, width: "100%", maxWidth: "min(720px, 100%)" }}>
            <FileChangeNotice
              files={[
                { path: "src/components/ui/NoticePanel.tsx", status: "modified", addedLines: 26, removedLines: 12 },
                { path: "src/components/ui/ListItem.tsx", status: "modified", addedLines: 8, removedLines: 6 },
                { path: "src/components/ui/Attachment.tsx", status: "modified", addedLines: 6, removedLines: 0 },
                { path: "src/components/ui/index.ts", status: "modified", addedLines: 2, removedLines: 0 },
              ]}
            />
          </div>
        </Row>
        <Divider dense />

        <Row label="上下文压缩 · 不是折叠卡片" hint="产品里它是独立的进度条（ChatCompactionProgress），没有标题栏也不可折叠；四态：进行中 / 已完成 / 已中断 / 失败">
          <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
            <CompactionProgress status="running" progress={42} detail="已整理 24 条消息 · 摘要已生成 1,280 字符" />
            <CompactionProgress status="completed" progress={100} detail="48,200 → 6,400 Token" />
            <CompactionProgress status="interrupted" detail="摘要未写入，历史保持原样" />
            <CompactionProgress status="failed" detail="上游返回 429，稍后自动重试" />
          </div>
        </Row>

      </Section>
      {/* ── 10 Attachment ──────────────────────────────────────────────────── */}
      <Section
        title="10 · Attachment 附件"
        desc="输入框里的附件卡：48px 高、12px 圆角，左侧 30px 图标块或 36px 图片预览，右侧文件名与大小。类型按扩展名判定，图片始终显示真实缩略图，其余按类型给占位图标"
        aside={<Badge variant="neutral">7 种类型</Badge>}
      >
        <Row label="图片 · 带缩略预览" hint="图片卡左侧是 36px 的 cover 裁剪预览，点击放大；没有数据时才退回图标">
          <div style={{ display: "flex", flexWrap: "wrap", gap: 8 }}>
            <Attachment
              name="device-screenshot.png"
              kind="image"
              detail="248 KB"
              src="data:image/svg+xml;utf8,%3Csvg xmlns='http://www.w3.org/2000/svg' width='36' height='36'%3E%3Crect width='36' height='36' fill='%236366f1'/%3E%3Ccircle cx='18' cy='14' r='7' fill='%23fff' opacity='.8'/%3E%3Crect y='24' width='36' height='12' fill='%23fff' opacity='.55'/%3E%3C/svg%3E"
              onOpen={() => toast.info("打开图片预览")}
              onRemove={() => undefined}
            />
            <Attachment
              name="logcat-filter.png"
              kind="image"
              detail="92 KB"
              src="data:image/svg+xml;utf8,%3Csvg xmlns='http://www.w3.org/2000/svg' width='36' height='36'%3E%3Crect width='36' height='36' fill='%230ea5e9'/%3E%3Crect x='5' y='7' width='26' height='3' fill='%23fff' opacity='.85'/%3E%3Crect x='5' y='14' width='18' height='3' fill='%23fff' opacity='.7'/%3E%3Crect x='5' y='21' width='22' height='3' fill='%23fff' opacity='.5'/%3E%3C/svg%3E"
              onOpen={() => toast.info("打开图片预览")}
              onRemove={() => undefined}
            />
            <Attachment name="pending.png" kind="image" detail="待加载" onRemove={() => undefined} />
          </div>
        </Row>

        <Divider dense />

        <Row label="文件与目录 · 占位图标" hint="目录 / 代码 / 文本 / 压缩包 / JSON / 文档，各用专属图标">
          <div style={{ display: "flex", flexWrap: "wrap", gap: 8 }}>
            <Attachment name="src/components/ui" kind="directory" detail="工作目录" onRemove={() => undefined} />
            <Attachment name="Attachment.tsx" kind="code" detail="5.1 KB" onRemove={() => undefined} />
            <Attachment name="build.log" kind="text" detail="1.2 MB" onRemove={() => undefined} />
            <Attachment name="release-v2.apk" kind="archive" detail="18.4 MB" onRemove={() => undefined} />
            <Attachment name="package.json" kind="json" detail="2.1 KB" onRemove={() => undefined} />
            <Attachment name="spec.pdf" kind="document" detail="640 KB" onRemove={() => undefined} />
          </div>
        </Row>

        <Divider dense />

        <Row label="按文件名自动判定" hint="attachmentKindOf() 与产品拖放分类同一套扩展名规则">
          <div style={{ display: "flex", flexWrap: "wrap", gap: 8 }}>
            {["MainActivity.kt", "styles.css", "AndroidManifest.xml", "device.tar.gz", "config.json", "README.md", "gradle.properties"].map((file) => (
              <Attachment key={file} name={file} kind={attachmentKindOf(file)} detail="自动判定" onRemove={() => undefined} />
            ))}
          </div>
        </Row>

        <Divider dense />

        <Row label="过程状态" hint="正在准备 / 准备失败可重试 / 只读无移除">
          <div style={{ display: "flex", flexWrap: "wrap", gap: 8 }}>
            <Attachment name="recording.mp4" kind="text" preparing />
            <Attachment name="broken.apk" kind="archive" error="读取失败：文件被占用" onRetry={() => toast.info("重试中")} />
            <Attachment name="readonly.json" kind="json" detail="1.0 KB" />
          </div>
        </Row>

        <Divider dense />

        <Row label="tile 形态" hint="纯图片缩略图，用于更紧凑的贴图区；hover 才显示移除">
          <div style={{ display: "flex", gap: 10, alignItems: "flex-start" }}>
            <Attachment
              name="a.png"
              kind="image"
              shape="tile"
              src="data:image/svg+xml;utf8,%3Csvg xmlns='http://www.w3.org/2000/svg' width='56' height='56'%3E%3Crect width='56' height='56' fill='%236366f1'/%3E%3C/svg%3E"
              onOpen={() => toast.info("打开预览")}
              onRemove={() => undefined}
            />
            <Attachment
              name="b.png"
              kind="image"
              shape="tile"
              src="data:image/svg+xml;utf8,%3Csvg xmlns='http://www.w3.org/2000/svg' width='56' height='56'%3E%3Crect width='56' height='56' fill='%23e11d48'/%3E%3C/svg%3E"
              onOpen={() => toast.info("打开预览")}
            />
            <Attachment name="c.png" kind="image" shape="tile" onRemove={() => undefined} />
          </div>
        </Row>
      </Section>
      <Section title="操作审批菜单 · AccessModeMenu" desc="对话输入框使用同一个共享组件，主题背景不透明；完全访问的确认由业务侧处理。">
        <AccessModeMenu inline value={accessMode} onChange={setAccessMode} />
      </Section>
      {/* ── 11 ProgressRing ────────────────────────────────────────────────── */}
      <Section
        title="11 · ProgressRing 环形进度"
        desc="输入框右下角的上下文用量环：常态灰、70% 转琥珀、90% 转红；点开显示总用量条与三项 token 明细。"
        aside={<Badge variant="primary">对话页同源</Badge>}
      >
        <Row label="四档阈值" hint="未知 / 正常 / 警告 70% / 危险 90%">
          <div style={{ display: "flex", gap: 24, alignItems: "center", flexWrap: "wrap" }}>
            <ProgressRing percent={null} label="占用未知" />
            <ProgressRing percent={28} label="已用 28%" />
            <ProgressRing percent={74} label="已用 74%" />
            <ProgressRing percent={93} label="已用 93%" />
            <ProgressRing percent={64} busy label="正在读取" />
          </div>
        </Row>

        <Divider dense />

        <Row label="展开明细" hint="点击环形打开用量条与图例，Esc 或点外部关闭">
          <div style={{ display: "flex", gap: 24, alignItems: "center", paddingTop: 8 }}>
            <ProgressRing
              percent={76}
              label="上下文已用 76%"
              valueText="~763.5K / 1M"
              segments={[
                { id: "system", label: "系统提示词", percent: 0.39, valueText: "~3.9K" },
                { id: "tools", label: "工具定义", percent: 1.37, valueText: "~13.7K" },
                { id: "messages", label: "对话消息", percent: 74.59, valueText: "~745.9K" },
              ]}
            />
            <span style={{ fontSize: 12, color: "var(--text-tertiary)" }}>点左侧环形查看明细面板</span>
          </div>
        </Row>
      </Section>
      {/* ── 12 UsageMetrics ────────────────────────────────────────────────── */}
      <Section
        title="12 · UsageMetrics 用量指标条"
        desc="输入框底部那一行：输出速度 / 本轮输入 / 本轮输出 / 缓存命中 / 余额 / 本轮消耗。10px 字号、上方一条发丝线、各项基线对齐，余额是按钮因为要重新查询"
        aside={<Badge variant="primary">对话页同源</Badge>}
      >
        <Row label="完整指标" hint="数值由调用方格式化；未上报的显示破折号而不是 0">
          <div style={{ display: "flex", flexDirection: "column", gap: 18 }}>
            <UsageMetrics
              metrics={[
                { id: "speed", label: "输出速度", value: "62 token/s" },
                { id: "input", label: "本轮输入", value: "12.4k" },
                { id: "output", label: "本轮输出", value: "1.8k" },
                { id: "cache", label: "缓存命中", value: "78%", detail: "最近一次模型请求的缓存命中率" },
              ]}
              balance={{ label: "余额", value: "¥42.60", detail: "点击重新查询账户余额" }}
              onRefreshBalance={() => toast.info("正在查询余额")}
              cost={{ label: "本轮消耗", value: "≈¥0.012", detail: "根据用量估算，≈ 表示估算值" }}
            />
            <UsageMetrics
              metrics={[
                { id: "speed", label: "输出速度", value: null, detail: "供应商尚未返回可计算速度的生成时间" },
                { id: "input", label: "本轮输入", value: "8.2k" },
                { id: "output", label: "本轮输出", value: null, detail: "供应商尚未返回输出用量" },
                { id: "cache", label: "缓存命中", value: null, detail: "尚未上报" },
              ]}
              balance={{ label: "余额", value: null, refreshing: true }}
              cost={{ label: "本轮消耗", value: null, detail: "本轮尚未收到消费数据，暂不计为零" }}
            />
          </div>
        </Row>
      </Section>

      {/* ── 13 FilePreviewPanel ────────────────────────────────────────────── */}
      <Section
        title="13 · FilePreviewPanel 文件预览"
        desc="只读预览外壳：标题 + 工具条、源码/差异切换、+N −N 计数、文件内搜索、路径页脚。内容区是插槽，语法高亮与文件读取留在业务侧"
        aside={<Badge variant="neutral">shell only</Badge>}
      >
        <Row label="差异模式 · 带计数" hint="标题栏四个工具：搜索 / 复制 / 重读 / 关闭">
          <div style={{ height: 300, display: "flex" }}>
            <FilePreviewPanel
              path="src/components/ui/NoticePanel.tsx"
              mode="diff"
              showModes
              onModeChange={() => undefined}
              added={26}
              removed={12}
              searchable
              onClose={() => toast.info("关闭预览")}
              onCopy={() => toast.info("已复制文件内容")}
              onRetry={() => toast.info("重新读取")}
            >
              <FileDiffView rows={[
                { kind: "meta", text: "@@ -12,2 +12,3 @@" },
                { kind: "removed", text: "  margin-inline: -8px;", oldLine: 12 },
                { kind: "added", text: "  padding: 8px 6px;", newLine: 12 },
                { kind: "added", text: "  border-radius: 0;", newLine: 13 },
                { kind: "context", text: "  display: flex;", oldLine: 13, newLine: 14 },
              ]} />
            </FilePreviewPanel>
          </div>
        </Row>

        <Divider dense />

        <Row label="源码模式 · 带搜索栏" hint="搜索栏展开后显示 当前/总数，Enter 下一个、Shift+Enter 上一个">
          <div style={{ height: 260, display: "flex" }}>
            <FilePreviewPanel
              path="src/views/Chat/ComposerNoticeDock.tsx"
              searchable
              query="notice"
              matchIndex={1}
              matches={7}
              onMoveMatch={() => undefined}
              onClose={() => toast.info("关闭预览")}
            >
              <pre style={{ margin: 0, padding: 10, font: "11.5px/1.7 var(--font-mono)", color: "var(--text-secondary)" }}>
{`const order: NoticeKind[] = [
  "permission", "question", "plan",
  "background", "queue", "compaction", "files",
];`}
              </pre>
            </FilePreviewPanel>
          </div>
        </Row>
      </Section>
      {/* ── 14 FileChangeCard ──────────────────────────────────────────────── */}
      <Section
        title="14 · FileChangeCard 回复后的文件改动"
        desc="AI 回复结束后沉淀在消息末尾的卡片：已编辑 N 个文件 + 总增删，带撤销 / 查看变更；默认只列 3 行，可展开。和 10 节里「回复过程中」的紧凑行是两张不同的卡"
        aside={<Badge variant="primary">回复后</Badge>}
      >
        <Row label="常规 · 多于 3 个文件" hint="超过 3 个才出现「再显示 N 个文件」">
          <div style={{ display: "flex", flexDirection: "column", gap: 16, width: "100%" }}>
            <FileChangeCard
              files={[
                { path: "src/components/ui/FileChangeCard.tsx", status: "created", addedLines: 128, removedLines: 0 },
                { path: "src/components/ui/NoticePanel.tsx", status: "modified", addedLines: 26, removedLines: 12 },
                { path: "src/components/ui/ListItem.tsx", status: "modified", addedLines: 8, removedLines: 6 },
                { path: "src/components/ui/Attachment.tsx", status: "modified", addedLines: 6, removedLines: 0 },
                { path: "src/components/ui/Thumbnail.tsx", status: "deleted", addedLines: 0, removedLines: 64 },
                { path: "src/components/ui/index.ts", status: "modified", addedLines: 2, removedLines: 0 },
              ]}
              onOpenFile={() => toast.info("打开文件预览")}
              onReview={() => toast.info("查看变更")}
              onUndo={() => toast.info("撤销本轮全部改动")}
            />
          </div>
        </Row>

        <Divider dense />

        <Row label="少量改动 · 无展开" hint="3 个以内不显示展开按钮；删除的文件路径带删除线">
          <div style={{ display: "flex", flexDirection: "column", gap: 16, width: "100%" }}>
            <FileChangeCard
              files={[
                { path: "src/views/Components/index.tsx", status: "modified", addedLines: 12, removedLines: 4 },
                { path: "src/styles/index.css", status: "modified", addedLines: 2, removedLines: 0 },
              ]}
              onOpenFile={() => toast.info("打开文件预览")}
              onReview={() => toast.info("查看变更")}
              onUndo={() => toast.info("撤销本轮全部改动")}
            />
          </div>
        </Row>

        <Divider dense />

        <Row label="已撤销部分" hint="revertedPaths 里的文件不再列出，表头数量与总增删随之下降">
          <div style={{ display: "flex", flexDirection: "column", gap: 16, width: "100%" }}>
            <FileChangeCard
              files={[
                { path: "a.ts", status: "modified", addedLines: 10, removedLines: 2 },
                { path: "b.ts", status: "modified", addedLines: 5, removedLines: 1 },
                { path: "c.ts", status: "created", addedLines: 3, removedLines: 0 },
              ]}
              revertedPaths={["b.ts"]}
              onReview={() => toast.info("查看变更")}
            />
          </div>
        </Row>
      </Section>
    </div>
  );
};
/** Provider lives here so the gallery can demonstrate live toasts. */
export const ComponentsView: React.FC = () => (
  <ToastProvider>
    <ComponentsGallery />
  </ToastProvider>
);

export default ComponentsView;
