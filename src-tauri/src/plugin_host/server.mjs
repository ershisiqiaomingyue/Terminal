import { createServer } from 'node:http';
import { readFile, writeFile, rename } from 'node:fs/promises';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { createInterface } from 'node:readline';
import { Context } from '@deepseek-ai/cordis';
import { Session } from '@deepseek-ai/dsh-session';
import { SessionEvents } from './events.mjs';
import { mountServices } from './services.mjs';
import { mountModelProviders } from './model-providers.mjs';

const [directory, data, key] = process.argv.slice(2);
const descriptor = JSON.parse(await readFile(join(directory, 'descriptor.json'), 'utf8'));
const ctx = new Context();
const routes = [], modules = new Map(), configs = new Map();
const eventBridge = new SessionEvents(ctx, (id, header) => new Session(id, undefined,
  { version: 4, id, createdAt: Date.now(), isSeeded: false, title: header.title }));
const services = await mountServices(ctx);
let origin;
const send = (value) => process.stdout.write(JSON.stringify(value) + '\n');
const modelProviders = mountModelProviders(ctx, send);
console.log = (...args) => console.error(...args);
const optionsPath = join(data, 'options.json');
let options = {};
try { options = JSON.parse(await readFile(optionsPath, 'utf8')); } catch (error) { if (error.code !== 'ENOENT') throw error; }
ctx.provide('webServer', { register(route) { routes.push(route); return () => { const i = routes.indexOf(route); if (i >= 0) routes.splice(i, 1); }; } });
function plain(value) {
  if (value && typeof value.get === 'function') return plain(value.get());
  if (Array.isArray(value)) return value.map(plain);
  if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, plain(item)]));
  return value;
}
const descriptions = () => descriptor.entries.map(row => {
  const module = modules.get(row.id), config = configs.get(row.id);
  const base = module.Config ? plain(module.Config(row.config ?? {})) : row.config ?? {};
  return { ns: row.id, user: options[row.id] ?? {}, base, config: { ...base, ...config } };
});
const settings = {
  describe: descriptions,
  configure: () => () => {},
  async update(ns, patch, ops) {
    const config = configs.get(ns), module = modules.get(ns);
    if (!config) throw new Error('未知插件设置');
    const user = structuredClone({ ...(options[ns] ?? {}), ...patch });
    for (const op of ops ?? []) {
      if (!Array.isArray(op.path) || !op.path.length || op.path.some(key => typeof key !== 'string' || ['__proto__','constructor','prototype'].includes(key))) throw new Error('设置路径无效');
      let target = user;
      for (const key of op.path.slice(0, -1)) { if (!target[key] || typeof target[key] !== 'object') target[key] = {}; target = target[key]; }
      const key = op.path.at(-1);
      if (op.op === 'set') target[key] = op.value; else if (op.op === 'unset') delete target[key]; else throw new Error('设置操作无效');
    }
    const row = descriptor.entries.find(row => row.id === ns);
    const proposed = { ...row.config, ...user };
    if (module.Config) module.Config(proposed);
    const nextOptions = { ...options, [ns]: user };
    await writeFile(optionsPath + '.tmp', JSON.stringify(nextOptions, null, 2));
    await rename(optionsPath + '.tmp', optionsPath);
    options = nextOptions;
    for (const key of Object.keys(config)) delete config[key];
    Object.assign(config, proposed);
    ctx.emit('loader/volatile-update', Object.keys(proposed).map(name => [name]));
    ctx.emit('settings/update', ns, patch);
  },
};
ctx.provide('settings', settings);
ctx.provide('sessions', { get: id => eventBridge.sessions.get(id)?.session, list: () => [...eventBridge.sessions.values()].map(value => value.session) });
const failed = [];
for (const row of descriptor.entries) {
  try {
    const module = await import(pathToFileURL(row.entry).href);
    // 宿主没装的服务不再让插件装不上：先补占位实现，只把缺口写进日志。
    services.fill(module.inject);
    if (typeof module.apply !== 'function') throw new Error('未提供 apply 入口');
    const config = { ...(row.config ?? {}), ...(options[row.id] ?? {}) };
    modules.set(row.id, module); configs.set(row.id, config);
    await module.apply(ctx, config);
  } catch (error) {
    // 一个插件装配失败不再连坐整个宿主：把错误留在 runtime.log，其余插件照常加载。
    // 只有全部插件都失败时才中止启动，让界面上的失败原因保持完整。
    failed.push(`${row.name}：${error?.message ?? error}`);
    console.error(`[plugin] ${row.name} 加载失败：${error?.stack ?? error?.message ?? error}`);
  }
}
if (failed.length && failed.length === descriptor.entries.length) throw new Error(failed.join('\n'));
async function jsonBody(req) {
  let text = ''; for await (const chunk of req) { text += chunk; if (text.length > 2 * 1024 * 1024) throw new Error('请求过大'); }
  return text ? JSON.parse(text) : {};
}
function json(res, code, value) { res.writeHead(code, { 'content-type': 'application/json; charset=utf-8' }); res.end(JSON.stringify(value)); }
const server = createServer(async (req, res) => {
  try {
    if (req.headers.host !== new URL(origin).host) return json(res, 403, { error: 'Host rejected' });
    const url = new URL(req.url, origin);
    if (!['GET', 'HEAD'].includes(req.method) && req.headers.origin !== origin && req.headers['x-plugin-key'] !== key) return json(res, 403, { error: 'Origin rejected' });
    if (url.pathname.startsWith('/_host/')) {
      if (req.headers['x-plugin-key'] !== key && url.searchParams.get('key') !== key) return json(res, 403, { error: 'Host key required' });
      if (url.pathname === '/_host/settings') {
        if (req.method === 'POST') { const body = await jsonBody(req); await settings.update(body.ns, body.patch, body.ops); }
        return json(res, 200, descriptions());
      }
      if (url.pathname === '/_host/rpc' && req.method === 'POST') {
        const body = await jsonBody(req);
        res.on('close', () => { if (!res.writableEnded) services.cancel(body.id); });
        return json(res, 200, body.method === 'provider-quota'
          ? await modelProviders.quota(body.params?.providerId)
          : await services.rpc(body.method, body.params, body.id));
      }
      if (url.pathname === '/_host/client.js') { res.setHeader('content-type', 'text/javascript'); return res.end(await readFile(join(directory, 'client-host.js'))); }
      if (url.pathname === '/_host/client.css') { res.setHeader('content-type', 'text/css'); return res.end(await readFile(join(directory, 'client-host.css'))); }
      if (url.pathname === '/_host/plugin.js' && descriptor.client) { res.setHeader('content-type', 'text/javascript'); return res.end(await readFile(descriptor.client)); }
      if (url.pathname === '/_host/frame') {
        res.setHeader('content-type', 'text/html; charset=utf-8'); res.setHeader('cache-control', 'no-store');
        return res.end(`<!doctype html><html><head><meta charset="utf-8"><style>html,body{margin:0;background:transparent;font:14px system-ui}button,input,select{font:inherit}#plugin-settings{display:none;position:fixed;inset:70px max(20px,10vw);z-index:2147483646;background:var(--bg-surface,#fff);color:var(--text-primary,#222);border-radius:12px;padding:20px;overflow:auto}#plugin-settings[data-open]{display:block}#plugin-settings-close{float:right}
:root{--plugin-settings-base:#fff}html[data-theme=dark]{--plugin-settings-base:#12151c}body:has(#plugin-settings[data-open]){background:#0b102038}#plugin-settings{background:linear-gradient(var(--bg-surface,#fff),var(--bg-surface,#fff)),var(--plugin-settings-base)}html[data-settings-inline] body{background:transparent}html[data-settings-inline] #plugin-settings{position:static!important;inset:auto!important;padding:0!important;border:0!important;border-radius:0!important;overflow:hidden!important;box-shadow:none!important}html[data-settings-inline] #plugin-settings-close{display:none!important}
</style></head><body><div id="plugin-settings"><button id="plugin-settings-close">关闭</button><div id="plugin-settings-content"></div></div><script>window.__PLUGIN_HOST__=${JSON.stringify({ key, name: descriptor.name, settingsOnly: url.searchParams.get('view') === 'settings', namespaces: descriptor.entries.map(row => row.id) }).replaceAll('<', '\\u003c')}</script><script>if(window.__PLUGIN_HOST__.settingsOnly)document.documentElement.dataset.settingsInline='1';</script><script src="/_host/client.js?key=${key}"></script><script src="/_host/plugin.js?key=${key}"></script></body></html>`);
      }
      return json(res, 404, { error: 'Host route not found' });
    }
    if (url.pathname === '/favicon.ico') { res.writeHead(204); return res.end(); }
    const route = routes.find(route => route.kind === 'exact' && route.path === url.pathname)
      ?? routes.filter(route => route.kind === 'prefix' && (url.pathname === route.path || url.pathname.startsWith(route.path + '/'))).sort((a,b) => b.path.length-a.path.length)[0];
    if (!route) return json(res, 404, { error: 'Route not found' });
    await route.handler(req, res);
  } catch (error) { if (!res.headersSent) json(res, 500, { error: error.message }); else res.end(); }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
origin = `http://127.0.0.1:${server.address().port}`;
send({ type: 'ready', origin });
const input = createInterface({ input: process.stdin });
input.on('line', line => { try { const message = JSON.parse(line); if (message.type === 'event') eventBridge.accept(message.payload); if (message.type === 'cancel') services.cancel(message.id); if (message.type === 'release') void services.release(message.scope); if (message.type === 'shutdown') void stop(); } catch (error) { console.error(error.message); } });
let stopping;
function stop() { return stopping ??= (async () => { modelProviders.close(); await services.stop(); await ctx.fiber.dispose(); server.close(); process.exit(0); })(); }
input.on('close', () => void stop());
process.on('SIGTERM', () => void stop());
