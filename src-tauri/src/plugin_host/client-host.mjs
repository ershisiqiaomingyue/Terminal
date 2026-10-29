import * as React from 'react';
import * as ReactDOM from 'react-dom';
import * as ReactClient from 'react-dom/client';
import * as JSX from 'react/jsx-runtime';
import * as Store from '@deepseek-ai/dsh-client-store';
import * as Primitives from '@deepseek-ai/dsh-client-ui-primitives';
import { Context } from '@deepseek-ai/cordis';

const host = window.__PLUGIN_HOST__;
const stylesheet = document.createElement('link');
stylesheet.rel = 'stylesheet'; stylesheet.href = `/_host/client.css?key=${encodeURIComponent(host.key)}`;
document.head.appendChild(stylesheet);
// DSH theme aliases are a protocol-level mapping, shared by all loaded UIs.
const themeAliases = {
  'bg-layer-2': 'bg-surface', 'bg-layer-3': 'bg-card', 'bg-module-platform': 'bg-surface',
  'interactive-bg-hover': 'bg-hover', 'label-primary': 'text-primary', 'label-secondary': 'text-secondary',
  'label-tertiary': 'text-tertiary', 'label-dimmed': 'text-tertiary', 'border-l2': 'border-subtle',
  'brand-primary': 'accent-primary', 'brand-primary-new-colorprimary-new-color': 'accent-primary',
  'state-error-primary': 'danger-text', 'state-warn-primary': 'warning-text',
};
for (const [alias, token] of Object.entries(themeAliases)) document.documentElement.style.setProperty(`--dsw-alias-${alias}`, `var(--${token})`);
const ctx = new Context();
const dictionaries = new Map(), forms = new Map(), slots = [];
let parentOrigin, disposed = false;
const publish = (type, payload) => parent.postMessage({ channel: 'android-tool-plugin', key: host.key, type, payload }, parentOrigin ?? '*');
const sessionList = Store.createSnapshotStore({ byId: {} });
function translate(ns) {
  return (key, params = {}) => {
    const dictionary = dictionaries.get(ns)?.zh ?? {};
    const text = dictionary[key] ?? key.split('.').reduce((value, part) => value?.[part], dictionary) ?? key;
    return String(text).replace(/\{(\w+)\}/g, (_, name) => String(params[name] ?? `{${name}}`));
  };
}
ctx.provide('locale', { register(ns, value) { dictionaries.set(ns, value); return () => dictionaries.delete(ns); }, bind: translate });
ctx.provide('sessions', { list: sessionList });
ctx.provide('uiWorkspace', { openSession: id => publish('open-session', { id }) });
ctx.provide('connection', { state: Store.createSnapshotStore({ status: 'connected' }) });
ctx.provide('remote', { request: async (path, body) => request(path, body) });
const headers = { 'content-type': 'application/json', 'x-plugin-key': host.key };
async function request(path, body) {
  const response = await fetch(path, { headers, ...(body === undefined ? {} : { method: 'POST', body: JSON.stringify(body) }) });
  const result = await response.json();
  if (!response.ok) throw new Error(result.error ?? `HTTP ${response.status}`);
  return result;
}
const description = Store.createSnapshotStore({ view: { namespaces: [] } });
function form(ns) {
  if (forms.has(ns)) return forms.get(ns);
  const store = Store.createSnapshotStore({ status: 'loading', writable: false, mode: 'host' });
  const value = { getSnapshot: store.getSnapshot, subscribe: store.subscribe, async mutate(ops) {
    await request('/_host/settings', { ns, ops }); await refreshSettings(); return true;
  }, set: (name, value) => valueFor(ns, name, value), refresh: () => refreshSettings(), ensure: () => refreshSettings(), store };
  forms.set(ns, value); return value;
}
async function refreshSettings() {
  const rows = await request('/_host/settings');
  description.set({ view: { namespaces: rows } });
  for (const row of rows) form(row.ns).store.set({ status: 'ready', value: row.config, base: row.base, user: row.user,
    writable: true, mode: 'host', revision: JSON.stringify(row.user) });
}
async function valueFor(ns, name, value) { await request('/_host/settings', { ns, patch: { [name]: value } }); await refreshSettings(); }
ctx.provide('configForms', { get: form, describe: () => description,
  whileServed(namespaces, callback) { return namespaces.every(ns => host.namespaces.includes(ns)) ? ctx.effect(callback) : () => {}; },
});
ctx.provide('slots', {
  inject(_name, callback) { return ctx.effect(callback); },
  register(spec, component) { const entry = { spec, component }; slots.push(entry); publish('slots', slots.map(({ spec }) => ({ id: spec.id, name: spec.name, label: spec.label?.() ?? spec.id }))); queueMicrotask(() => { if (settingsPanel.hasAttribute('data-open')) openSettings(); }); return () => { const index = slots.indexOf(entry); if (index >= 0) slots.splice(index, 1); }; },
});
const modules = { react: React, 'react-dom': ReactDOM, 'react-dom/client': ReactClient, 'react/jsx-runtime': JSX,
  '@deepseek-ai/cordis': { Context }, '@deepseek-ai/dsh-client-store': Store, '@deepseek-ai/dsh-client-runtime/client': Store };
modules['@deepseek-ai/dsh-client-ui-primitives'] = Primitives;
window.__ModuleLoader__ = { load({ id, factory }) {
  try {
    const module = factory(name => { if (!(name in modules)) throw new Error(`未提供浏览器宿主模块：${name}`); return modules[name]; });
    modules[id] = module;
    const missing = (module.inject ?? []).filter(name => !ctx.get(name, false));
    if (missing.length) throw new Error(`缺少浏览器宿主服务：${missing.join('、')}`);
    if (typeof module.apply !== 'function') throw new Error('插件未提供浏览器 apply 入口');
    module.apply(ctx);
    publish('ready', { id });
    if (host.settingsOnly) openSettings();
  } catch (error) { publish('error', { message: error.message }); void teardown(); }
} };
let settingsRoot;
const settingsPanel = document.getElementById('plugin-settings');
function openSettings() {
  void refreshSettings().catch(error => console.error('无法刷新插件设置', error));
  const entries = slots.filter(entry => ['settings.section', 'plugins.bundle.config'].includes(entry.spec.name));
  if (!settingsRoot) settingsRoot = ReactClient.createRoot(document.getElementById('plugin-settings-content'));
  function Slot({ entry }) {
    const face = entry.spec.inject?.() ?? {};
    const hooks = Object.fromEntries(Object.entries(face.hooks ?? {}).map(([name, source]) => [
      'use' + name[0].toUpperCase() + name.slice(1), (select = value => value) => select(React.useSyncExternalStore(source.subscribe, source.getSnapshot)),
    ]));
    return React.createElement(entry.component, { ...face, ...hooks, t: translate(entry.spec.locale), ctx });
  }
  settingsRoot.render(React.createElement(React.Fragment, {}, entries.map(entry => React.createElement(Slot, { key: entry.spec.id ?? entry.spec.key, entry }))));
  settingsPanel.dataset.open = '1'; settingsPanel.querySelector('button')?.focus(); publish('modal', { open: true }); updateRegions();
}
function closeSettings() { if (host.settingsOnly) return; delete settingsPanel.dataset.open; publish('modal', { open: false }); updateRegions(); }
document.getElementById('plugin-settings-close').onclick = closeSettings;
document.addEventListener('keydown', event => { if (event.key === 'Escape') closeSettings(); });
window.addEventListener('message', event => {
  if (event.source !== parent || event.data?.channel !== 'android-tool-plugin' || event.data.key !== host.key) return;
  if (parentOrigin && parentOrigin !== event.origin) return;
  parentOrigin = event.origin;
  const { type, payload } = event.data;
  if (type === 'theme') {
    for (const [name, value] of Object.entries(payload.tokens ?? {})) document.documentElement.style.setProperty(name, value);
    document.documentElement.dataset.theme = payload.theme;
    document.documentElement.style.colorScheme = payload.colorScheme;
  }
  if (type === 'sessions') sessionList.set({ byId: payload.byId });
  if (type === 'settings') openSettings();
  if (type === 'shutdown') void teardown();
});
// A transparent iframe only captures pointer input over actual plugin surfaces.
// Rectangles describe the plugin's actual visible content, including popovers.
let lastRegions = '', frame;
function updateRegions() {
  if (disposed || frame) return;
  frame = requestAnimationFrame(() => {
    frame = undefined;
    const rects = [];
    for (const element of document.querySelectorAll('body *')) {
      const style = getComputedStyle(element);
      if (style.display === 'none' || style.visibility === 'hidden' || style.pointerEvents === 'none') continue;
      const rect = element.getBoundingClientRect();
      if (rect.width > 0 && rect.height > 0 && rect.width < innerWidth * .98 && rect.height < innerHeight * .98) rects.push({ x: rect.x, y: rect.y, width: rect.width, height: rect.height });
    }
    const serialized = JSON.stringify(rects);
    if (serialized !== lastRegions) { lastRegions = serialized; publish('regions', rects); }
  });
}
const observer = new MutationObserver(updateRegions);
const sizeObserver = new ResizeObserver(() => {
  if (host.settingsOnly) publish('settings-height', { height: Math.ceil(settingsPanel.getBoundingClientRect().height) });
});
sizeObserver.observe(document.getElementById('plugin-settings-content'));
observer.observe(document.body, { childList: true, subtree: true, attributes: true });
window.addEventListener('resize', updateRegions);
document.addEventListener('pointerdown', () => publish('capture', { active: true }), true);
document.addEventListener('pointerup', () => { publish('capture', { active: false }); updateRegions(); }, true);
document.addEventListener('pointercancel', () => publish('capture', { active: false }), true);
async function teardown() {
  if (disposed) return; disposed = true; observer.disconnect(); sizeObserver.disconnect();
  if (frame) cancelAnimationFrame(frame);
  settingsRoot?.unmount(); await ctx.fiber.dispose(); publish('regions', []);
}
window.addEventListener('pagehide', () => void teardown(), { once: true });
void refreshSettings().catch(error => publish('error', { message: error.message }));
publish('boot', {});
window.addEventListener('error', event => publish('error', { message: event.message }));
window.addEventListener('unhandledrejection', event => publish('error', { message: String(event.reason?.message ?? event.reason) }));
