/**
 * 命令块 AI 的 Tauri 依赖装配（`terminalAiService` 的真实 IO 实现）。
 *
 * 单独成文件的原因：`terminalAiService.ts` 必须能在 `node
 * --experimental-strip-types` 下被单测直接 import，而 `@/platform/runtime` 与
 * `@/platform/api` 顶层就 `import { invoke } from "@tauri-apps/api/core"`，
 * 在非 Tauri 环境加载即抛错。把真实依赖隔离在这里，两边都干净。
 *
 * 使用方（终端视图 / BlockAiCard）只需要 `import { terminalAiDeps } from
 * "./terminalAiDeps.ts"`，不必知道底层通道。
 */
import { listAgentProviders } from "@/platform/api";
import { invoke, listen } from "@/platform/runtime";
import type { BlockAiDeps, TerminalProviderLike } from "./terminalAiService.ts";

export function createTerminalAiDeps(): BlockAiDeps {
  return {
    invoke: (command, args) => invoke(command, args),
    listen: (event, handler) => listen(event, handler),
    listProviders: async () => {
      const providers = await listAgentProviders();
      // 只需 id / model 两个字段；显式映射一遍，避免把整个供应商对象
      // （含 baseUrl、quotaUrls）带进服务层的类型空间。
      return providers.map((provider): TerminalProviderLike => ({
        id: provider.id,
        name: provider.name,
        model: provider.model,
        models: provider.models?.map((model) => ({
          id: model.id,
          name: model.name,
          supportsReasoning: model.supportsReasoning ?? null,
        })),
      }));
    },
    now: () => Date.now(),
  };
}

let shared: BlockAiDeps | null = null;

/** 复用同一份依赖，避免每张卡片各建一次。 */
export function terminalAiDeps(): BlockAiDeps {
  if (!shared) shared = createTerminalAiDeps();
  return shared;
}
