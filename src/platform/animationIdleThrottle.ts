/**
 * 窗口不可见时暂停所有 CSS 动画。
 *
 * 为什么需要：合成器对「正在跑 animation 的元素」是逐帧产帧的，即使窗口已经被
 * 最小化或完全遮挡也照旧。实测本应用最小化时 GPU 进程仍吃满一个核——用户把窗口
 * 收进任务栏、或者专心看别的窗口时，这些帧全是白烧。
 *
 * 做法：把 html[data-app-idle] 挂上去，由 index.css 里的一条规则统一
 * `animation-play-state: paused`。恢复可见时移除属性，动画从原处继续。
 */

const IDLE_ATTR = "data-app-idle";

function sync(): void {
  const root = document.documentElement;
  if (document.hidden) root.setAttribute(IDLE_ATTR, "1");
  else root.removeAttribute(IDLE_ATTR);
}

export function installAnimationIdleThrottle(): void {
  sync();
  document.addEventListener("visibilitychange", sync);
}
