/**
 * 设备上下文采集（开发方案 9.2.2 / 任务 T3-3 / 验收项 F45）。
 *
 * 只在 `block.targetKind === "adb"` 的会话里采集：解释一条「adb shell xxx」的
 * 输出，不知道机器是什么型号、有没有 root，给出的建议经常是错的（例如对
 * userdebug 的机器建议 `verity disable`）。
 *
 * 设计上把 `invoke` 作为参数注入而不是直接 import `@/platform/runtime`：
 * 1. 本文件的逻辑必须能在 Node 单测里跑（`node --experimental-strip-types`），
 *    而 `@tauri-apps/api/core` 在非 Tauri 环境下加载即失败；
 * 2. 三个后端命令任一失败都不该阻断解释流程——降级成"该字段未知"即可，
 *    所以这里用 `Promise.allSettled` 而不是 `Promise.all`。
 *
 * 隐私（9.2.2 明文要求）：默认不发送序列号原文，只发 `redactSerial(serial)`
 * （保留前 4 位）。用户可在设置里显式打开"发送完整设备标识"。
 */
import { redactSerial } from "../blocks/ansiText.ts";

/** 后端 `get_adb_device_info` 的返回结构（对齐 src/views/AdbDevices/index.tsx:83）。 */
export interface AndroidDeviceInfo {
  androidVersion: string;
  sdk: string;
  manufacturer: string;
  model: string;
  buildFingerprint: string;
}

/** 后端 `get_adb_device_status` 的返回结构。 */
export interface AndroidDeviceStatus {
  batteryLevel: string;
  batteryStatus: string;
  batteryTemperature: string;
  storage: string;
  abi: string;
  securityPatch: string;
  systemServicesReady: boolean;
  systemServicesMessage: string;
}

/** 后端 `get_adb_privilege_status` 的返回结构。 */
export interface AndroidPrivilegeStatus {
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

/**
 * 注入的 IPC 通道。签名与 `src/platform/runtime.ts` 的 `invoke` 一致，
 * 只是为了本模块可单测而不直接依赖它。
 */
export type DeviceInvoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

/** 采集结果。字段全部可选：任一后端命令失败时对应字段为 undefined。 */
export interface DeviceContext {
  /** 已按脱敏策略处理过的序列号（可直接进 prompt）。 */
  serial?: string;
  /** `true` 表示 `serial` 不是原文，提示词里要告知模型这一点。 */
  serialRedacted: boolean;
  model?: string;
  manufacturer?: string;
  androidVersion?: string;
  sdk?: string;
  buildFingerprint?: string;
  buildType?: string;
  isRoot?: boolean;
  selinux?: string;
  verifiedBootState?: string;
  debuggable?: boolean;
  abi?: string;
  securityPatch?: string;
  storage?: string;
  batteryLevel?: string;
  /** 采集到的命令里失败的那几个，用于让模型知道"有些信息我不知道"。 */
  unavailable: string[];
  /** 采集时刻（毫秒），调用方可据此缓存。 */
  collectedAt: number;
}

export interface CollectOptions {
  invoke: DeviceInvoke;
  /** 设备工作目录（来自 OSC 7），可选。 */
  cwd?: string;
  /** 是否发送序列号原文；默认 false（9.2.2）。 */
  includeFullSerial?: boolean;
  /** 注入时钟，单测用。 */
  now?: () => number;
}

/** 一次采集最多并发三个命令，任一失败都不抛。 */
export async function collectDeviceContext(
  serial: string | undefined,
  options: CollectOptions,
): Promise<DeviceContext> {
  const { invoke } = options;
  const now = options.now ?? (() => Date.now());
  const context: DeviceContext = {
    serial: serial ? (options.includeFullSerial ? serial.trim() : redactSerial(serial)) : undefined,
    serialRedacted: Boolean(serial) && !options.includeFullSerial,
    unavailable: [],
    collectedAt: now(),
  };
  if (!serial) return context;

  const [info, status, privilege] = await Promise.allSettled([
    invoke<AndroidDeviceInfo>("get_adb_device_info", { serial }),
    invoke<AndroidDeviceStatus>("get_adb_device_status", { serial }),
    invoke<AndroidPrivilegeStatus>("get_adb_privilege_status", { serial }),
  ]);

  if (info.status === "fulfilled" && info.value) {
    const value = info.value;
    context.model = value.model || undefined;
    context.manufacturer = value.manufacturer || undefined;
    context.androidVersion = value.androidVersion || undefined;
    context.sdk = value.sdk || undefined;
    context.buildFingerprint = value.buildFingerprint || undefined;
  } else {
    context.unavailable.push("设备型号与系统版本");
  }

  if (status.status === "fulfilled" && status.value) {
    const value = status.value;
    context.abi = value.abi || undefined;
    context.securityPatch = value.securityPatch || undefined;
    context.storage = value.storage || undefined;
    context.batteryLevel = value.batteryLevel || undefined;
  } else {
    context.unavailable.push("电池与存储状态");
  }

  if (privilege.status === "fulfilled" && privilege.value) {
    const value = privilege.value;
    context.isRoot = Boolean(value.isRoot);
    context.buildType = value.buildType || undefined;
    context.selinux = value.selinux || undefined;
    context.verifiedBootState = value.verifiedBootState || undefined;
    context.debuggable = Boolean(value.debuggable);
  } else {
    context.unavailable.push("root 与构建类型");
  }

  return context;
}

/** 把上下文渲染成提示词里的固定段落；没有任何字段时返回空串（不占 token）。 */
export function formatDeviceContext(context: DeviceContext | undefined): string {
  if (!context) return "";
  const lines: string[] = [];
  const push = (label: string, value: string | number | boolean | undefined) => {
    if (value === undefined || value === "") return;
    lines.push(`- ${label}：${String(value)}`);
  };
  push("序列号", context.serial ? `${context.serial}${context.serialRedacted ? "（已脱敏，非完整序列号）" : ""}` : undefined);
  push("厂商", context.manufacturer);
  push("型号", context.model);
  push("Android 版本", context.androidVersion);
  push("SDK", context.sdk);
  push("构建类型", context.buildType);
  if (context.isRoot !== undefined) push("是否 root", context.isRoot ? "是" : "否");
  push("SELinux", context.selinux);
  push("verified boot", context.verifiedBootState);
  if (context.debuggable !== undefined) push("userdebug/可调试", context.debuggable ? "是" : "否");
  push("ABI", context.abi);
  push("安全补丁", context.securityPatch);
  push("存储", context.storage);
  push("电量", context.batteryLevel);
  // `unavailable` 在类型上是必填，但它来自跨 IPC 的采集结果，历史存档/外部
  // 调用方可能给不全；这里用可选链兜住，避免一条缺字段的上下文把整个 AI
  // 请求打成异常（提示词少一行远比请求失败好）。
  if (context.unavailable?.length) push("未能采集", context.unavailable.join("、"));
  if (!lines.length) return "";
  return `## 设备上下文\n${lines.join("\n")}`;
}
