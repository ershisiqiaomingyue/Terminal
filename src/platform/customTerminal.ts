/**
 * 自研终端内核（基于 libghostty-vt WASM 的定制化封装）的加载与实例创建。
 *
 * 关键约束（阶段 0 POC 实测，见开发方案附录 B / V1）：
 * - wasm 必须显式给路径。`init()` 不接受参数，其内部的 `load()` 默认用
 *   `new URL('../core.wasm', import.meta.url)`，在 Vite dev（依赖预打包）与
 *   生产打包下都会解析失败（`expected magic word 00 61 73 6d`）。
 * - 推荐路径：`?url` 导入资源，`load(url)` 得到实例，再 `new Terminal({ core })`。
 */
import { FitAddon, Ghostty, Terminal } from "@aurigax-ai/ghostty-web";
import wasmUrl from "@aurigax-ai/ghostty-web/ghostty-vt.wasm?url";
import type { AppFitAddon, AppTerminal, TerminalOptions } from "./terminalAdapter.ts";
import { currentDimForeground, rewriteDimSequences } from "./terminalAnsiDim.ts";
import { recordCoreLoad, setTerminalRendererType } from "./terminal/metrics.ts";

type CoreInstance = Awaited<ReturnType<typeof Ghostty.load>>;
type CoreTerminalOptions = ConstructorParameters<typeof Terminal>[0];

let corePromise: Promise<CoreInstance> | null = null;

/** 加载一次 wasm 并复用同一个内核实例（模块内单例）。 */
export function ensureCoreReady(): Promise<CoreInstance> {
  if (!corePromise) {
    // 埋点 11.3：wasm 初始化耗时只在真正加载的这一次记，命中单例不重复计时。
    const startedAt = Date.now();
    corePromise = Ghostty.load(wasmUrl)
      .then((instance) => {
        recordCoreLoad(Date.now() - startedAt);
        return instance;
      })
      .catch((error: unknown) => {
        // 失败不缓存：下次调用可以重试（例如资源被临时清理）。
        corePromise = null;
        throw error;
      });
  }
  return corePromise;
}

/** 用自研内核创建终端；调用前无需自己 await，这里会等 wasm 就绪。 */
export async function createCustomTerminal(options: TerminalOptions): Promise<AppTerminal> {
  const core = await ensureCoreReady();
  // 键名由内核库决定（它只认 `ghostty`），与我们对内叫它「自研内核」无关。
  const terminal = new Terminal({ ...options, ghostty: core } as unknown as CoreTerminalOptions);
  const target = terminal as unknown as AppTerminal;
  // 埋点 11.3：内核自报实际渲染后端（设置里选 canvas 失败回落到 webgl 时要能看出来）。
  setTerminalRendererType(target.rendererType ?? "unknown");
  // dim 可读性补偿（6.6 / V5）：内核把 SGR 2 固定按 50% 混色，亮色主题下必然低于
  // 4.5:1，且 minimumContrastRatio 不作用于 dim；这里在写入前改写成显式前景色。
  // 暗色主题下 currentDimForeground() 返回 null，等于原样透传。
  const write = target.write.bind(target);
  target.write = ((data: string, callback?: () => void) => {
    write(rewriteDimSequences(data, currentDimForeground()), callback);
  }) as AppTerminal["write"];
  return target;
}

/** 自研内核版本的 FitAddon（`proposeDimensions` / `fit` 与 xterm 同名）。 */
export function createCustomFitAddon(): AppFitAddon {
  return new FitAddon() as unknown as AppFitAddon;
}

/** 供测试或诊断使用：wasm 是否已经加载完成。 */
export function isCoreReady(): boolean {
  return corePromise !== null;
}

export { wasmUrl as coreWasmUrl };
