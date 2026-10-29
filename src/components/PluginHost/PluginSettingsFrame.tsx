import { useEffect, useRef, useState } from 'react';
import type { HostPlugin } from '@/platform/plugins';
import { buildPluginThemePayload } from './themePayload';

export function PluginSettingsFrame({ plugin }: { plugin: HostPlugin }) {
  const frame = useRef<HTMLIFrameElement>(null);
  const [height, setHeight] = useState(420);
  const [error, setError] = useState('');
  useEffect(() => {
    const origin = new URL(plugin.frameUrl!).origin;
    function syncTheme() {
      const computed = getComputedStyle(document.documentElement);
      frame.current?.contentWindow?.postMessage({ channel: 'android-tool-plugin', key: plugin.bridgeKey, type: 'theme',
        payload: buildPluginThemePayload(computed, document.documentElement.dataset.theme) }, origin);
    }
    function receive(event: MessageEvent) {
      if (event.source !== frame.current?.contentWindow || event.origin !== origin || event.data?.channel !== 'android-tool-plugin' || event.data.key !== plugin.bridgeKey) return;
      const { type, payload } = event.data;
      if (type === 'boot' || type === 'ready') syncTheme();
      if (type === 'settings-height' && Number.isFinite(payload?.height)) setHeight(Math.min(12000, Math.max(120, payload.height)));
      if (type === 'error') setError(String(payload?.message || '插件设置加载失败'));
    }
    const observer = new MutationObserver(syncTheme);
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ['class', 'style', 'data-theme', 'data-color-theme'] });
    window.addEventListener('message', receive);
    return () => { observer.disconnect(); window.removeEventListener('message', receive); };
  }, [plugin.frameUrl, plugin.bridgeKey]);
  return <div className="plugin-inline-settings">
    {error && <p className="setting-desc" role="alert">{error}</p>}
    <iframe ref={frame} title={`${plugin.name} 设置`} src={`${plugin.frameUrl}&view=settings`}
      style={{ height }} allow="clipboard-read; clipboard-write" sandbox="allow-scripts allow-same-origin allow-downloads allow-popups allow-popups-to-escape-sandbox" />
  </div>;
}
