/**
 * 会话完成提示窗的 URL 判定。
 *
 * 单独放在这里而不是跟组件放一起：`Startup` 只需要判断「当前是不是提示窗」，
 * 从组件文件里 import 会把组件和它的 CSS 一起拽进主包。
 */
export const isNoticeWindow = () => new URLSearchParams(window.location.search).has("notice");
