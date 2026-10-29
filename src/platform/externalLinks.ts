import { isTauri } from "./runtime";

/** 只有这些协议允许交给系统处理，其余一律拦在应用内。 */
const EXTERNAL_PROTOCOLS = /^(?:https?|mailto|tel):/i;
/** 模块加载时就抓住原生实现，避免被下面的护栏覆盖后自我递归。 */
const nativeWindowOpen = typeof window === "undefined" ? undefined : window.open.bind(window);

export function isExternalLink(url: string) {
  return EXTERNAL_PROTOCOLS.test(url);
}

/**
 * 用系统默认浏览器打开链接。
 *
 * 应用的主窗口是 `decorations(false)` 的无边框窗口，一旦 WebView 内部导航到
 * 外部页面就没有标题栏、没有返回按钮，用户只能重启程序（曾经这样卡死过一次）。
 * 因此所有外部链接都必须走这里，绝不能在应用内打开。
 */
export async function openExternalLink(url: string) {
  if (!isExternalLink(url)) return false;
  if (isTauri()) {
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(url);
    return true;
  }
  nativeWindowOpen?.(url, "_blank", "noopener,noreferrer");
  return true;
}

let installed = false;

/**
 * 兜底护栏：捕获阶段拦截任何 `<a href>` 点击与 `window.open` 调用。
 *
 * 消息正文里的 Markdown 链接、插件说明、诊断报告等都可能渲染出链接，
 * 逐个组件改造容易漏；这里在 document 捕获阶段统一改写，保证没有遗漏。
 * 应用内部导航全部走按钮/状态切换，不使用 `<a>`，所以拦截是安全的。
 */
export function installExternalLinkGuard() {
  if (installed) return;
  installed = true;

  document.addEventListener("click", (event) => {
    const target = event.target instanceof Element ? event.target : null;
    const anchor = target?.closest("a[href]");
    if (!anchor) return;
    const href = anchor.getAttribute("href") || "";
    if (!href || href.startsWith("#")) return;
    event.preventDefault();
    event.stopPropagation();
    if (isExternalLink(href)) {
      void openExternalLink(href).catch((error) => console.error("[external-link] 打开失败", href, error));
    }
  }, true);

  // Tauri 里 window.open 默认会尝试新建 WebView 窗口，同样改走外部浏览器。
  const nativeOpen = window.open.bind(window);
  window.open = ((url?: string | URL, target?: string, features?: string) => {
    const href = typeof url === "string" ? url : url ? String(url) : "";
    if (href && isExternalLink(href)) {
      void openExternalLink(href).catch((error) => console.error("[external-link] 打开失败", href, error));
      return null;
    }
    return nativeOpen(url as string, target, features);
  }) as typeof window.open;
}
