// Optional package-owned providers. Credentials are sent only over native stdout.
export function mountModelProviders(ctx, publish) {
  const entries = new Map();
  let previous = '';
  function refresh() {
    const providers = [...entries].flatMap(([id, entry]) => {
      const value = entry.snapshot();
      return value ? [{ ...value, id }] : [];
    });
    const serialized = JSON.stringify(providers);
    if (serialized !== previous) { previous = serialized; publish({ type: 'providers', providers }); }
  }
  ctx.provide('modelProviders', {
    register(entry) {
      if (!/^[a-zA-Z0-9_-]{1,64}$/.test(entry.id) || entries.has(entry.id) || typeof entry.snapshot !== 'function') throw new Error('供应商注册无效或重复');
      entries.set(entry.id, entry); refresh();
      return () => { if (entries.get(entry.id) === entry) { entries.delete(entry.id); refresh(); } };
    },
    refresh,
  });
  const timer = setInterval(refresh, 1000);
  timer.unref();
  return {
    async quota(id) {
      const entry = entries.get(id);
      if (!entry?.snapshot() || typeof entry.quota !== 'function') throw new Error('供应商未连接或不支持查询额度');
      try { return await entry.quota(); } finally { refresh(); }
    },
    close() { clearInterval(timer); entries.clear(); },
  };
}
