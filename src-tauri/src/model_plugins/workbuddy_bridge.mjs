// WorkBuddy adapter for Android Tool. Upstream protocol/auth implementation:
// https://github.com/corrinehu/dsh-workbuddy-connect (MIT).
import { createServer } from 'node:http';
import { randomBytes, timingSafeEqual } from 'node:crypto';
import { mkdir, access, readFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { createInterface } from 'node:readline';

const MAX_BODY_BYTES = 64 * 1024 * 1024;
const TIMEOUT_MS = 45_000;
const bounded = (promise) => {
  let timer;
  return Promise.race([promise, new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error('WorkBuddy 请求超时，请稍后刷新')), TIMEOUT_MS);
  })]).finally(() => clearTimeout(timer));
};
const json = (res, status, value) => {
  res.writeHead(status, { 'Content-Type': 'application/json' });
  res.end(JSON.stringify(value));
};
const fail = (res, status, message) => json(res, status, { error: { message, type: 'workbuddy_error' } });
const sameSecret = (actual, expected) => {
  const a = Buffer.from(actual || '');
  const b = Buffer.from(expected);
  return a.length === b.length && timingSafeEqual(a, b);
};

export async function configureWorkBuddyElectron(config, env = process.env, platform = process.platform) {
  for (const [field, variable, folder, filename] of [
    ['electronFileCn', 'WORKBUDDY_ELECTRON_BIN', 'WorkBuddy', 'WorkBuddy.exe'],
    ['electronFileAi', 'WORKBUDDY_AI_ELECTRON_BIN', 'WorkBuddyAI', 'WorkBuddyAI.exe'],
  ]) {
    if (config[field]?.trim()) { env[variable] = config[field].trim(); continue; }
    if (env[variable]?.trim() || platform !== 'win32') continue;
    const roots = [env.LOCALAPPDATA && join(env.LOCALAPPDATA, 'Programs'), env.ProgramFiles, env['ProgramFiles(x86)']].filter(Boolean);
    for (const root of roots) {
      const candidate = join(root, folder, filename);
      try {
        await Promise.all([access(candidate), access(join(root, folder, 'version')), access(join(root, folder, 'resources', 'app.asar'))]);
        env[variable] = candidate;
        break;
      } catch { /* A custom location can be selected in plugin settings. */ }
    }
  }
}

export async function loadWorkBuddyApi(directory) {
  const base = join(directory, 'node_modules/dsh-workbuddy-connect/lib');
  const entry = await readFile(join(base, 'index.js'), 'utf8');
  // Upstream wires automatic Windows registry discovery through its private
  // factory. The public CredentialStore constructor defaults to no discovery.
  const binding = /import\s*\{([^}]*\batRestKeyProviderFor\b[^}]*)\}\s*from\s*["'](\.\/[^"']+)["']/.exec(entry);
  const alias = binding && /\b(\w+)\s+as\s+atRestKeyProviderFor\b/.exec(binding[1]);
  if (!binding || !alias || binding[2].includes('..')) throw new Error('WorkBuddy 插件的凭据发现接口已变化，请使用兼容版本');
  const api = await import(pathToFileURL(join(base, 'index.js')).href);
  const internal = await import(pathToFileURL(join(base, binding[2])).href);
  if (typeof internal[alias[1]] !== 'function') throw new Error('WorkBuddy 安装路径发现接口不可用');
  const manifest = JSON.parse(await readFile(join(base, '../package.json'), 'utf8'));
  return { ...api, packageVersion: manifest.version, atRestKeyProviderFor: internal[alias[1]] };
}

export function workBuddyModelView(api, source, now = Date.now(), observation) {
  const model = api.modelWithCurrentPromotion?.(source, now) || source;
  const brand = /^(?:hy\d|hunyuan)/i.test(model.id) ? 'hunyuan'
    : /^(deepseek|glm|minimax|kimi)(?:-|\d)/i.exec(model.id)?.[1]?.toLowerCase();
  const billing = model.billing;
  const normalized = api.normalizeCredits?.(billing?.credits) || billing?.credits;
  const rate = /^x(\d+(?:\.\d+)?)(?:\s+credits)?$/i.exec(normalized || '');
  const multiplier = rate ? Number(rate[1]) : null;
  return { id: model.id, name: model.name || model.id,
    iconBrand: brand ? `workbuddy-${brand}` : 'workbuddy',
    ...(billing ? { billing: {
      multiplier: !billing.rateUnknown && Number.isFinite(multiplier) ? multiplier : null,
      badges: [...new Set((billing.badges || []).filter((label) => typeof label === 'string' && label.trim()).map((label) => label.trim()))],
      free: billing.free === true, rateUnknown: billing.rateUnknown === true,
    } } : {}),
    protocol: 'open_ai_compatible', protocols: ['open_ai_compatible'],
    reasoningEfforts: model.reasoning?.supports === true
      ? [...(model.reasoning.supportedEfforts || (observation?.validation === 'validating' ? observation.efforts : []))] : [],
    capabilitySource: model.reasoning?.supportedEfforts?.length ? 'workbuddy-catalog' : observation?.validation === 'validating' ? 'workbuddy-probe' : 'workbuddy-catalog',
    contextWindow: model.contextWindow, supportsTools: true, supportsUsage: true,
    supportsVision: model.supportsImages === true, supportsReasoning: model.reasoning?.supports === true };
}

export async function createWorkBuddyBridge({ api, dataDir, config = {}, onSnapshot = () => {} }) {
  await mkdir(dataDir, { recursive: true });
  const secret = randomBytes(32).toString('base64url');
  const active = new Set();
  let closed = false;
  const runtimes = api.WORKBUDDY_VARIANTS.map((variant) => {
    const client = new api.WorkBuddyUpstreamClient();
    const store = new api.WorkBuddyCredentialStore({ variant,
      keyProvider: api.atRestKeyProviderFor?.(variant),
      desktopPath: variant.region === 'cn' ? config.authFileCn || undefined : config.authFileAi || undefined,
      ownPath: join(dataDir, variant.ownFilename), refresh: (credential) => client.refreshToken(credential) });
    return { variant, client, store, models: [], identity: '', fetchedAt: 0, probe: null, probing: '', probeErrors: new Map(),
      state: 'signed-out', message: '', account: '', credits: null, creditError: '', unlimited: false };
  });
  for (const r of runtimes) {
    if (!api.WorkBuddyProbeStore || !api.WorkBuddyProbeService) continue;
    r.probe = new api.WorkBuddyProbeService({
      store: new api.WorkBuddyProbeStore({ path: join(dataDir, r.variant.probeFilename || `${r.variant.region}-probes.json`), pluginVersion: api.packageVersion || '0.7.4' }),
      catalog: { current: () => r.models }, credentials: r.store, client: r.client,
      consent: () => false, account: () => r.state === 'signed-in' ? r.identity : undefined, region: r.variant.region,
      send: (modelId) => async (effort, signal) => {
        const identity = r.identity;
        const credential = await r.store.resolve();
        if (closed || `${credential.uid}:${credential.enterpriseId || ''}` !== identity) throw new Error('账号已切换，请重新检测');
        const controller = new AbortController();
        const abort = () => controller.abort(); signal.addEventListener('abort', abort, { once: true });
        if (signal.aborted) controller.abort();
        active.add(controller);
        try { return await r.client.probeEffort(credential, modelId, effort, controller.signal); }
        finally { signal.removeEventListener('abort', abort); active.delete(controller); }
      },
    });
  }
  const cleanError = (error) => {
    const message = String(error?.message || error);
    if (/no .*Electron binary|no usable .*Electron binary|application was found|expected app layout/.test(message)) return '未找到 WorkBuddy 程序，请在高级配置中选择已安装的程序文件，再保存并刷新。';
    if (/Electron binary is not available|key helper|encrypted-credential-unreadable/.test(message)) return '无法读取 WorkBuddy 加密登录状态，请确认程序路径，重新登录 WorkBuddy 后刷新。';
    return message
    .replace(/Bearer\s+\S+/gi, 'Bearer [已隐藏]')
    .replace(/((?:access|refresh)[_-]?token["'\s:=]+)[^\s,"'}]+/gi, '$1[已隐藏]').slice(0, 500);
  };
  const snapshot = () => ({ accounts: runtimes.map((r) => ({ id: r.variant.id,
    providerId: `plugin:workbuddy:${r.variant.region}`,
    name: r.variant.displayName, state: r.state, account: r.account, modelCount: r.models.length,
    credits: r.credits, unlimited: r.unlimited, message: r.message, creditError: r.creditError,
    reasoningModels: r.models.map((model) => {
      const record = r.probe?.recordFor(model.id);
      const view = workBuddyModelView(api, model, Date.now(), record);
      return { id: model.id, name: view.name, iconBrand: view.iconBrand, supportsReasoning: view.supportsReasoning,
        efforts: view.reasoningEfforts, source: model.reasoning?.supportedEfforts?.length ? 'catalog' : record ? 'probe' : 'unknown',
        validation: record?.validation || '', probedAt: record?.probedAtMs || null,
        canProbe: Boolean(r.probe && view.supportsReasoning && !model.reasoning?.supportedEfforts?.length),
        probing: r.probing === model.id, message: r.probeErrors.get(model.id) || '' };
    }) })),
    providers: runtimes.filter((r) => r.state === 'signed-in' && r.models.length).map((r) => ({
      id: `plugin:workbuddy:${r.variant.region}`, name: r.variant.displayName,
      kind: 'open_ai_compatible', brand: 'workbuddy', baseUrl: `${origin}/${r.variant.region}/v1`,
      model: r.models[0].id, models: r.models.map((model) => workBuddyModelView(api, model, Date.now(), r.probe?.recordFor(model.id))), token: secret })) });

  let refreshFlight;
  async function refresh(force = false) {
    if (refreshFlight) { await refreshFlight; if (!force) return snapshot(); }
    refreshFlight = Promise.all(runtimes.map(async (r) => {
      try {
        const current = await bounded(r.store.current());
        if (closed) return;
        if (!current) {
          r.state = 'signed-out'; r.models = []; r.identity = ''; r.account = '';
          r.credits = null; r.unlimited = false; r.creditError = ''; r.message = '请先在本机登录对应的 WorkBuddy 应用';
          return;
        }
        const identity = `${current.uid}:${current.enterpriseId || ''}`;
        const changed = identity !== r.identity;
        if (changed) { r.models = []; r.credits = null; r.unlimited = false; r.creditError = ''; r.fetchedAt = 0; r.probeErrors.clear(); }
        r.identity = identity; r.account = current.nickname || '已登录'; r.state = 'signed-in';
        if (force || changed || Date.now() - r.fetchedAt > 300_000) {
          r.fetchedAt = Date.now();
          const credential = await bounded(r.store.resolve());
          const controller = new AbortController();
          const timeout = setTimeout(() => controller.abort(), TIMEOUT_MS);
          let models;
          try { models = await r.client.fetchModels(credential, controller.signal); }
          finally { clearTimeout(timeout); }
          // Do not publish an old account's catalog after a desktop account switch.
          const latest = await bounded(r.store.current());
          if (!latest || `${latest.uid}:${latest.enterpriseId || ''}` !== identity || closed) {
            r.models = []; r.identity = ''; return;
          }
          r.models = [...models]; r.message = r.models.length ? '模型目录已同步' : '该账号没有可用模型';
        }
      } catch (error) {
        r.message = cleanError(error);
        if (!r.models.length) r.state = 'error';
      }
    })).then(() => { if (!closed) onSnapshot(snapshot()); }).finally(() => { refreshFlight = undefined; });
    await refreshFlight;
    if (force) await Promise.allSettled(runtimes.filter((r) => r.state === 'signed-in').map((r) => quota(`plugin:workbuddy:${r.variant.region}`)));
    return snapshot();
  }

  async function probe(providerId, modelId) {
    const r = runtimes.find((item) => `plugin:workbuddy:${item.variant.region}` === providerId);
    if (!r?.probe || r.state !== 'signed-in') throw new Error('请先登录 WorkBuddy 并刷新模型目录');
    if (!r.models.some((model) => model.id === modelId)) throw new Error('模型不在当前账号的目录中');
    if (r.probing) throw new Error('请等待当前模型检测完成');
    const identity = r.identity;
    r.probing = modelId; r.probeErrors.delete(modelId); onSnapshot(snapshot());
    try {
      const result = await r.probe.probe(modelId, true);
      const latest = await bounded(r.store.current());
      if (closed || !latest || `${latest.uid}:${latest.enterpriseId || ''}` !== identity) {
        await refresh(); throw new Error('账号已切换，本次检测结果未采用');
      }
      if (result.state !== 'ok') {
        const message = `检测未完成：${cleanError(result.reason)}`;
        r.probeErrors.set(modelId, message); throw new Error(message);
      }
      return result;
    } finally {
      r.probing = ''; if (!closed) onSnapshot(snapshot());
      if (!closed && r.identity === identity) await quota(providerId).catch(() => {});
    }
  }

  async function quota(providerId) {
    const r = runtimes.find((item) => `plugin:workbuddy:${item.variant.region}` === providerId);
    if (!r) throw new Error('找不到这个 WorkBuddy 模型分组');
    try {
      const credential = await bounded(r.store.resolve());
      const identity = `${credential.uid}:${credential.enterpriseId || ''}`;
      if (identity !== r.identity) { await refresh(); throw new Error('WorkBuddy 账号已切换，请重新查询积分'); }
      const credits = await bounded(r.client.fetchCredits(credential));
      const latest = await bounded(r.store.current());
      if (!latest || `${latest.uid}:${latest.enterpriseId || ''}` !== identity || closed) throw new Error('WorkBuddy 账号已切换，本次积分未采用');
      r.credits = credits.unlimited ? null : credits.total; r.unlimited = credits.unlimited === true; r.creditError = '';
      return { status: 'available', source: 'workbuddy', remainingAmount: r.credits,
        unit: '积分', message: r.unlimited ? '企业额度不限量' : 'WorkBuddy 剩余积分',
        checkedAt: String(Math.floor(Date.now() / 1000)) };
    } catch (error) { r.creditError = cleanError(error); throw new Error(r.creditError); }
    finally { if (!closed) onSnapshot(snapshot()); }
  }

  async function handle(req, res) {
    try {
      const host = req.headers.host || '';
      const allowedOrigin = !req.headers.origin || /^http:\/\/(127\.0\.0\.1|localhost)(:\d+)?$/.test(req.headers.origin);
      if (!/^127\.0\.0\.1:\d+$/.test(host) || !allowedOrigin) return fail(res, 403, '仅允许本机请求');
      if (!sameSecret(req.headers.authorization, `Bearer ${secret}`)) return fail(res, 401, '本地插件认证失败');
      const parts = /^\/(cn|global)\/v1\/(models|chat\/completions)$/.exec(req.url || '');
      const r = runtimes.find((item) => item.variant.region === parts?.[1]);
      if (!r) return fail(res, 404, '插件接口不存在');
      if (req.method === 'GET' && parts[2] === 'models') {
        return json(res, 200, { object: 'list', data: r.models.map((m) => ({ id: m.id, object: 'model', owned_by: 'workbuddy' })) });
      }
      if (req.method !== 'POST' || parts[2] !== 'chat/completions') return fail(res, 404, '插件接口不存在');
      if (!String(req.headers['content-type'] || '').toLowerCase().startsWith('application/json')) return fail(res, 415, '需要 JSON 请求');
      const chunks = []; let size = 0;
      for await (const chunk of req) {
        size += chunk.length;
        if (size > MAX_BODY_BYTES) return fail(res, 413, '消息体过大');
        chunks.push(chunk);
      }
      const input = JSON.parse(Buffer.concat(chunks).toString('utf8'));
      const model = r.models.find((m) => m.id === input.model);
      if (!model) return fail(res, 400, '该模型当前不可用，请刷新 WorkBuddy 模型目录');
      if (input.stream !== true) return fail(res, 400, 'WorkBuddy 仅支持流式请求');
      if (model.maxTokens && typeof input.max_tokens === 'number') input.max_tokens = Math.min(input.max_tokens, model.maxTokens);
      const capabilities = workBuddyModelView(api, model, Date.now(), r.probe?.recordFor(model.id));
      if (input.reasoning_effort && !capabilities.reasoningEfforts.includes(input.reasoning_effort)) delete input.reasoning_effort;
      const controller = new AbortController(); active.add(controller);
      const disconnect = () => { if (!res.writableEnded) controller.abort(); };
      res.on('close', disconnect);
      try {
        const credential = await bounded(r.store.resolve());
        if (`${credential.uid}:${credential.enterpriseId || ''}` !== r.identity) {
          await refresh(); return fail(res, 409, 'WorkBuddy 账号已切换，请确认模型后重试');
        }
        const result = await r.client.chatStream(credential, api.prepareChatBody(JSON.stringify(input)), controller.signal);
        if (!result.ok) return fail(res, ({ hard_credit: 402, soft_rate: 429, session_dead: 401, client: 400 })[result.kind] || 502, cleanError(result.message));
        if (!result.response.body) return fail(res, 502, 'WorkBuddy 未返回流式内容');
        res.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache', 'X-Accel-Buffering': 'no' });
        for await (const chunk of result.response.body) {
          if (controller.signal.aborted) break;
          if (!res.write(chunk)) await new Promise((done) => {
            const resume = () => { res.off('drain', resume); res.off('close', resume); done(); };
            res.once('drain', resume); res.once('close', resume);
          });
        }
        // Preserve upstream EOF/errors. Never invent [DONE] for a broken stream.
        res.end();
      } finally { active.delete(controller); res.off('close', disconnect); }
    } catch (error) {
      if (!res.headersSent) fail(res, 502, cleanError(error));
      else res.destroy();
    }
  }
  const server = createServer((req, res) => { void handle(req, res); });
  server.requestTimeout = 60_000;
  await new Promise((done, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', done); });
  const origin = `http://127.0.0.1:${server.address().port}`;
  await refresh(true);
  const timer = setInterval(() => { void refresh().catch(() => {}); }, 30_000);
  timer.unref();
  return { refresh, quota, probe, snapshot, async close() {
    closed = true; clearInterval(timer); for (const controller of active) controller.abort();
    await new Promise((done) => { server.close(done); server.closeAllConnections(); });
  } };
}

async function main() {
  const directory = resolve(process.argv[2]);
  const dataDir = resolve(process.argv[3]);
  const config = JSON.parse(process.argv[4] || '{}');
  await configureWorkBuddyElectron(config);
  const send = (value) => process.stdout.write(`${JSON.stringify(value)}\n`);
  const api = await loadWorkBuddyApi(directory);
  const bridge = await createWorkBuddyBridge({ api, dataDir, config, onSnapshot: (data) => send({ type: 'snapshot', data }) });
  send({ type: 'ready' });
  const reader = createInterface({ input: process.stdin });
  reader.on('line', (line) => {
    void (async () => {
      let request;
      try {
        request = JSON.parse(line);
        const data = request.method === 'quota' ? await bridge.quota(request.providerId)
          : request.method === 'probe' ? await bridge.probe(request.providerId, request.modelId)
          : request.method === 'refresh' ? await bridge.refresh(true) : bridge.snapshot();
        // Only the native host receives the private snapshot, including the ephemeral loopback token.
        send({ id: request.id, data });
      } catch (error) { send({ id: request?.id, error: String(error?.message || error).slice(0, 500) }); }
    })();
  });
  reader.on('close', () => { void bridge.close().finally(() => process.exit(0)); });
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch(() => { process.stderr.write('WorkBuddy 插件启动失败，请重新安装或检查运行环境\n'); process.exit(1); });
}
