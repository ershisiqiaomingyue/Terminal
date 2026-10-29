interface PluginEnvironment {
  id: string;
  name: string;
  localStatus: string;
  localMessage: string;
  supportsDownload: boolean;
}

type CheckEnvironment = (id: string) => Promise<PluginEnvironment>;

export async function missingPluginEnvironments(ids: string[], check: CheckEnvironment) {
  const statuses = await Promise.all([...new Set(ids)].map(check));
  const missing = statuses.filter((status) => status.localStatus !== "ready");
  const unsupported = missing.find((status) => !status.supportsDownload);
  if (unsupported) {
    throw new Error(`${unsupported.name} 不可用，请先在设置 → 环境与工具中选择本地运行环境。${unsupported.localMessage}`);
  }
  return missing;
}

/** Recheck immediately before installation; no plugin runs until every declared dependency is ready. */
export async function installWithPluginEnvironments<T>(ids: string[], services: {
  check: CheckEnvironment;
  download: CheckEnvironment;
  install: () => Promise<T>;
  allowDownload: boolean;
  onDownload?: (environment: PluginEnvironment) => void;
  onInstall?: () => void;
}): Promise<T> {
  for (const id of new Set(ids)) {
    const status = await services.check(id);
    if (status.localStatus === "ready") continue;
    if (!services.allowDownload || !status.supportsDownload) {
      throw new Error(`${status.name} 不可用，请先在环境与工具中安装或选择。`);
    }
    services.onDownload?.(status);
    const downloaded = await services.download(id);
    if (downloaded.localStatus !== "ready") {
      throw new Error(`${status.name} 安装未通过校验：${downloaded.localMessage}。插件未安装。`);
    }
  }
  services.onInstall?.();
  return services.install();
}
