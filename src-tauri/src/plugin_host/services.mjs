// DSH contracts are provided by the upstream services. Only the application
// session/permission boundary and the JSON transport belong to this host.
import { InvariantRegistry } from '@deepseek-ai/dsh-invariants';
import { Storage, storageBackendServiceKey } from '@deepseek-ai/dsh-storage';
import { apply as applyStorageDomain, Config as StorageDomainConfig } from '@deepseek-ai/dsh-storage-domain';
import { SessionPersistence } from '@deepseek-ai/dsh-session-persistence';
import { SessionProjectionRegistry } from '@deepseek-ai/dsh-session-projection';
import { AttachmentStore } from '@deepseek-ai/dsh-attachment';
import { CredentialProvider } from '@deepseek-ai/dsh-credentials';
import { ApprovalService } from '@deepseek-ai/dsh-user-approval';
import { SandboxProvider } from '@deepseek-ai/dsh-sandbox';
import { LlmRuntime } from '@deepseek-ai/dsh-llm';
import { PtcRuntime } from '@deepseek-ai/dsh-ptc-runtime';
import { HostConnectionService } from '@deepseek-ai/dsh-client-connection';
import { CommandRuntime } from '@deepseek-ai/dsh-commands';
import { ToolRuntime } from '@deepseek-ai/dsh-tools';
import { SystemPrompt, renderPrompt } from '@deepseek-ai/dsh-system-prompt';
import { LocalSubprocessRuntime } from '@deepseek-ai/dsh-subprocess-local';
import { createScope } from '@deepseek-ai/dsh-scope';
import { isAbsolute } from 'node:path';

// 上游真正提供的服务名（与各包的 `super(ctx, "...")` 一一对应）。插件声明的
// `inject` 只要落在这张表里，就一定拿得到实例，而不是拿到一个空壳。
const UPSTREAM_SERVICES = [
  ['invariants', () => InvariantRegistry, {}],
  ['storage', () => Storage, {}],
  ['sessionPersistence', () => SessionPersistence, {}],
  ['sessionProjections', () => SessionProjectionRegistry, {}],
  ['attachments', () => AttachmentStore, {}],
  ['credentials', () => CredentialProvider, {}],
  ['approval', () => ApprovalService, {}],
  ['sandbox', () => SandboxProvider, {}],
  ['llm', () => LlmRuntime, {}],
  ['ptcRuntime', () => PtcRuntime, {}],
  ['connection', () => HostConnectionService, {}],
  ['commands', () => CommandRuntime, {}],
  ['tools', () => ToolRuntime, { mode: 'native' }],
  ['systemPrompt', () => SystemPrompt, { includeHarnessIdentity: false }],
  ['subprocess', () => LocalSubprocessRuntime, {}],
];

// 存储域层（`ctx.storageDomain`）需要先有一个带 `kv` 面的后端。这里挂一个进程内
// 的 KV 后端：没有插件依赖跨重启的持久化，但工作区/会话投影等上层服务要求这个
// 服务必须存在，所以内存实现比缺服务更能让插件正常装配。
function mountMemoryBackend(ctx, units) {
  const backend = {
    kv: {
      async open(descriptor) {
        if (!units.has(descriptor.name)) {
          units.set(descriptor.name, {
            tables: Object.fromEntries((descriptor.tables ?? []).map(table => [table, {}])),
            global: null,
          });
        }
        const unit = units.get(descriptor.name);
        return {
          async loadAll() { return { tables: structuredClone(unit.tables), global: unit.global }; },
          async putRecord(table, key, value) { (unit.tables[table] ??= {})[key] = structuredClone(value); },
          async deleteRecord(table, key) { delete (unit.tables[table] ?? {})[key]; },
          async setGlobal(value) { unit.global = structuredClone(value); },
          async close() {},
        };
      },
    },
  };
  ctx.storage.backend.register('memory', backend);
  // 域层按 `storage.backend.<name>` 注入生命周期，缺了它会一直不激活。
  ctx.provide(storageBackendServiceKey('memory'), { name: 'memory' });
}

// 兜底：插件 inject 了一个本宿主确实没有的服务时，给一个惰性空壳而不是让整个
// 插件装配失败。调用方只有在真的调用它时才会拿到 undefined / 空数组，加载阶段
// 永远不会被自己的依赖卡住。
function fallbackService(name) {
  const target = { __hostFallback: name };
  return new Proxy(target, {
    get(store, key) {
      if (typeof key === 'symbol' || key in store) return Reflect.get(store, key);
      if (key === 'then' || key === 'toJSON') return undefined;
      if (/^(list|all|names|keys|entries|snapshot|values)$/i.test(key)) return () => [];
      if (/^(get|find)$/i.test(key)) return () => undefined;
      if (/^has$/i.test(key)) return () => false;
      return () => undefined;
    },
  });
}

// `ctx.plugin()` 返回一个 fiber；服务要等 fiber 建好才可读，所以整段必须是 async，
// 并且逐层 await——否则 `ctx.storage` 在挂 storage-domain 时还是 undefined。
export async function mountServices(ctx) {
  const agents = new Map(), calls = new Map();
  const units = new Map();

  await ctx.plugin(Storage);
  await ctx.plugin(InvariantRegistry);
  mountMemoryBackend(ctx, units);
  await ctx.plugin({ name: 'storage-domain', apply: applyStorageDomain, inject: ['storage'], Config: StorageDomainConfig },
    { backend: 'memory', routes: {} });
  for (const [name, load, config] of UPSTREAM_SERVICES) {
    if (name === 'storage' || name === 'invariants') continue;
    await ctx.plugin(load(), config);
  }

  // 应用侧的会话/权限边界：agent 由本宿主创建，sandboxPolicy 必须解析到它。
  ctx.provide('agents', { list: () => [...agents.values()].map(entry => entry.agent) });
  ctx.provide('sandboxPolicy', {
    resolve({ session }) {
      const entry = [...agents.values()].find(item => item.agent.session === session);
      if (!entry) throw new Error('会话已结束或未授权');
      return { mode: entry.mode, workspaceRoot: entry.cwd };
    },
  });

  // 插件声明了、但宿主既没装也没兜底的服务名，全部在这里补齐。
  const fill = (inject) => {
    const names = Array.isArray(inject) ? inject : Object.keys(inject?.required ?? {});
    for (const name of names) {
      if (ctx.get(name, false)) continue;
      ctx.provide(name, fallbackService(name));
      console.error(`[plugin] 宿主未提供 ${name}，已用占位实现兜底`);
    }
  };

  const cancel = id => calls.get(id)?.abort(new Error('请求已取消'));
  async function release(scope) {
    const entry = agents.get(scope);
    if (!entry) return;
    agents.delete(scope);
    for (const [id, controller] of calls) if (controller.scope === scope) cancel(id);
    ctx.emit('agent/disposed', { agent: entry.agent });
    await entry.boundary.dispose();
  }
  async function rpc(method, params, id) {
    if (method === 'prepare') {
      if (typeof params.scope !== 'string' || !params.sessionId || !isAbsolute(params.cwd ?? '')) throw new Error('插件工具需要已选择的本机工作目录');
      if (!['read-only', 'workspace-write', 'danger-full-access'].includes(params.mode)) throw new Error('权限模式无效');
      if (agents.has(params.scope)) throw new Error('插件会话已经存在');
      const agent = { id: params.scope, session: { id: params.sessionId, header: { cwd: params.cwd, origin: 'user' } } };
      const boundary = createScope(ctx, agent);
      agent.ctx = boundary.ctx;
      agents.set(params.scope, { agent, boundary, mode: params.mode, cwd: params.cwd });
      try {
        ctx.emit('agent/created', { agent });
        // Cordis injection creates its scoped layers synchronously; drain any
        // asynchronous apply callbacks before taking the public schema snapshot.
        await new Promise(resolve => setImmediate(resolve));
        const assembly = await ctx.get('systemPrompt').assemble({ scope: agent, agent });
        return { tools: assembly.tools, prompt: renderPrompt(assembly) };
      } catch (error) { await release(params.scope); throw error; }
    }
    if (method === 'release') { await release(params.scope); return {}; }
    if (method !== 'call') throw new Error('未知插件调用');
    const entry = agents.get(params.scope);
    if (!entry) throw new Error('插件会话已结束');
    const controller = new AbortController();
    controller.scope = params.scope;
    calls.set(id, controller);
    try {
      return await ctx.get('tools').execute({ name: params.name, arguments: params.arguments,
        callId: id, agent: entry.agent, signal: controller.signal });
    } finally { calls.delete(id); }
  }
  return { rpc, cancel, release, fill, async stop() { await Promise.all([...agents.keys()].map(release)); } };
}
