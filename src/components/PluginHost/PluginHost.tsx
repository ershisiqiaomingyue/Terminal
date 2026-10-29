import { useEffect, useRef, useState } from 'react';
import { listHostPlugins, type HostPlugin } from '@/platform/plugins';
import { invoke, isTauri, listen } from '@/platform/runtime';
import { CHAT_NAV_EVENT, dispatchChatNav, readChatNavSnapshot, readChatSessions } from '@/views/Chat/chatNavStore';
import { buildPluginThemePayload } from './themePayload';
import './plugin-host.css';

interface Region { x: number; y: number; width: number; height: number }
interface FrameState { plugin: HostPlugin; regions: Region[]; capture: boolean; modal: boolean }
export default function PluginHost() {
  const [plugins, setPlugins] = useState<HostPlugin[]>([]);
  const frames = useRef(new Map<string, HTMLIFrameElement>());
  const states = useRef(new Map<string, FrameState>());
  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false, sequence = 0;
    const stops: (() => void)[] = [];
    const reload = async () => {
      const revision = ++sequence;
      try {
        const next = await listHostPlugins();
        if (!disposed && revision === sequence) setPlugins(next.filter(plugin => plugin.enabled && plugin.frameUrl && plugin.bridgeKey));
      } catch (error) { console.error('无法加载插件', error); }
    };
    void reload();
    void listen('plugins-changed', () => { void reload(); }).then(stop => { if (disposed) stop(); else stops.push(stop); }).catch(error => console.error('无法订阅插件状态', error));
    return () => { disposed = true; ++sequence; stops.forEach(stop => stop()); };
  }, []);
  useEffect(() => {
    for (const plugin of plugins) {
      const current = states.current.get(plugin.id);
      if (!current || current.plugin.bridgeKey !== plugin.bridgeKey) states.current.set(plugin.id, { plugin, regions: [], capture: false, modal: false });
    }
    for (const id of states.current.keys()) if (!plugins.some(plugin => plugin.id === id)) states.current.delete(id);
    function send(plugin: HostPlugin, type: string, payload: unknown) {
      frames.current.get(plugin.id)?.contentWindow?.postMessage({ channel: 'android-tool-plugin', key: plugin.bridgeKey, type, payload }, new URL(plugin.frameUrl!).origin);
    }
    function syncTheme(plugin: HostPlugin) {
      const computed = getComputedStyle(document.documentElement);
      send(plugin, 'theme', buildPluginThemePayload(computed, document.documentElement.dataset.theme));
    }
    function syncSessions(plugin: HostPlugin) {
      const active = readChatNavSnapshot().sessionId;
      send(plugin, 'sessions', { byId: Object.fromEntries(readChatSessions().map(session => [session.id,
        { id: session.id, title: session.title, retainedBy: { mainView: active === session.id ? 1 : 0 } }])) });
    }
    function updateClip(state: FrameState) {
      const frame = frames.current.get(state.plugin.id);
      if (!frame) return;
      if (state.capture || state.modal) { frame.style.clipPath = 'none'; return; }
      const path = state.regions.map(rect => {
        const inset = 12, x = Math.floor(rect.x - inset), y = Math.floor(rect.y - inset);
        const width = Math.ceil(rect.width + inset * 2), height = Math.ceil(rect.height + inset * 2);
        return `M${x} ${y}h${width}v${height}h-${width}Z`;
      }).join(' ');
      frame.style.clipPath = path ? `path('${path}')` : 'inset(100%)';
    }
    const onMessage = (event: MessageEvent) => {
      const state = [...states.current.values()].find(state => frames.current.get(state.plugin.id)?.contentWindow === event.source);
      if (!state || event.origin !== new URL(state.plugin.frameUrl!).origin || event.data?.channel !== 'android-tool-plugin' || event.data.key !== state.plugin.bridgeKey) return;
      const { type, payload } = event.data;
      if (type === 'boot' || type === 'ready') { syncTheme(state.plugin); syncSessions(state.plugin); }
      if (type === 'regions' && Array.isArray(payload)) state.regions = payload.filter((value: Region) => value && [value.x, value.y, value.width, value.height].every(Number.isFinite));
      if (type === 'capture') state.capture = payload?.active === true;
      if (type === 'modal') state.modal = payload?.open === true;
      if (type === 'regions' || type === 'capture' || type === 'modal') updateClip(state);
      if (type === 'open-session' && typeof payload?.id === 'string') {
        window.dispatchEvent(new CustomEvent('android-tool-plugin-open-chat'));
        dispatchChatNav({ type: 'open-session', id: payload.id });
      }
      if (type === 'error') {
        const message = String(payload?.message ?? '插件界面加载失败');
        void invoke('report_plugin_error', { id: state.plugin.id, bridgeKey: state.plugin.bridgeKey, message }).catch(error => console.error('无法记录插件错误', error));
      }
    };
    const settings = (event: Event) => {
      const id = (event as CustomEvent<{ id: string }>).detail?.id;
      const state = states.current.get(id);
      if (state) send(state.plugin, 'settings', {});
    };
    const themeObserver = new MutationObserver(() => plugins.forEach(syncTheme));
    themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ['class', 'style', 'data-theme', 'data-color-theme'] });
    const sessions = () => plugins.forEach(syncSessions);
    window.addEventListener('message', onMessage);
    window.addEventListener('android-tool-plugin-settings', settings);
    window.addEventListener(CHAT_NAV_EVENT, sessions);
    return () => {
      themeObserver.disconnect();
      window.removeEventListener('message', onMessage);
      window.removeEventListener('android-tool-plugin-settings', settings); window.removeEventListener(CHAT_NAV_EVENT, sessions);
    };
  }, [plugins]);
  return <>{plugins.map(plugin => <iframe key={`${plugin.id}:${plugin.bridgeKey}`} title={`${plugin.name} 插件`}
    className="plugin-host-frame" src={plugin.frameUrl!} allow="clipboard-read; clipboard-write" sandbox="allow-scripts allow-same-origin allow-downloads allow-popups allow-popups-to-escape-sandbox"
    ref={frame => { if (frame) frames.current.set(plugin.id, frame); else frames.current.delete(plugin.id); }} />)}</>;
}
