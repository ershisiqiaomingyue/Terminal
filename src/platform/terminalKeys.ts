/**
 * 终端按键兜底（对应用户实测问题：键入第一个按键时电脑「响一声」，且该键丢失）。
 *
 * 背景（已核对自研内核所依赖的 WASM 包源码）：
 * - `lib/terminal.ts:433-435` 的 `open()` 会把宿主设成 `contenteditable="true"`，
 *   且 `terminal.ts:950-952` 的 `focus()` 聚焦的是宿主 div 而不是隐藏的 textarea。
 *   → 这两点已在 `terminalAdapter.ts` 的 `dropTerminalContentEditable` / `focusTerminalInput` 中修掉，
 *     正常输入路径现在是「焦点在隐藏 textarea → beforeinput → onData」。
 * - 但 `lib/input-handler.ts:266-320` 同时给宿主 container 设了 `tabindex=0`，
 *   所以焦点仍可能落在宿主 div 上（例如 `terminal.focus()` 的兜底分支）。
 *   此时 `handleKeyDown` 里 `mapKeyCode()` 返回 null 的键、纯修饰键都不会 preventDefault，
 *   按键没有被任何输入控件消费，WebView2 会把它们交给宿主窗口，Windows 于是播放系统提示音。
 *
 * 兜底规则（刻意收得很窄，避免影响正常输入）：
 * **只有 `event.target` 就是宿主 container 本身**（不是 textarea、不是 canvas）时才介入。
 * 这种情况下按键本来就不会产生任何字符（没有可编辑元素承接），`preventDefault()`
 * 唯一的作用就是阻止它冒泡出 WebView 触发系统提示音，不会吞掉任何真实输入。
 * 焦点在 textarea 上时（正常态）我们完全不介入，让浏览器的 beforeinput 通路照常工作。
 */

const FUNCTION_KEYS = new Set([
  "F1",
  "F2",
  "F3",
  "F4",
  "F5",
  "F6",
  "F7",
  "F8",
  "F9",
  "F10",
  "F11",
  "F12",
]);

const MODIFIER_KEYS = new Set(["Shift", "Control", "Alt", "Meta", "CapsLock", "NumLock", "ScrollLock"]);

/** 是否要吞掉这次按键的默认行为（true＝preventDefault，避免系统提示音）。 */
export function shouldSwallowKeydown(event: KeyboardEvent, container: HTMLElement): boolean {
  // 焦点不在宿主本身（例如在隐藏 textarea）→ 交给浏览器/内核正常处理，绝不介入。
  if (event.target !== container) return false;
  // 内核已经消费，别重复处理。
  if (event.defaultPrevented) return false;
  // 组合输入期一定要保留默认行为，否则候选词上不了屏。
  if (event.isComposing || event.keyCode === 229) return false;
  if (MODIFIER_KEYS.has(event.key)) return false;
  if (FUNCTION_KEYS.has(event.code)) return false;
  const command = event.ctrlKey || event.metaKey;
  if (command && event.code === "KeyR") return false;
  if (command && event.shiftKey && (event.code === "KeyI" || event.code === "KeyJ" || event.code === "KeyC")) {
    return false;
  }
  return true;
}

/** 在终端宿主上挂按键兜底，返回解绑函数。 */
export function guardTerminalKeydown(container: HTMLElement): () => void {
  const onKeyDown = (event: KeyboardEvent) => {
    if (shouldSwallowKeydown(event, container)) event.preventDefault();
  };
  // keypress 是独立于 keydown 的另一条默认行为通路，同样只在焦点位于宿主本身时兜底。
  const onKeyPress = (event: KeyboardEvent) => {
    if (event.target !== container) return;
    if (event.isComposing) return;
    if (!event.defaultPrevented) event.preventDefault();
  };
  container.addEventListener("keydown", onKeyDown);
  container.addEventListener("keypress", onKeyPress);
  return () => {
    container.removeEventListener("keydown", onKeyDown);
    container.removeEventListener("keypress", onKeyPress);
  };
}
