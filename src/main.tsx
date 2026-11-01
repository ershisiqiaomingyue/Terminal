import ReactDOM from "react-dom/client";
import { Startup } from "./Startup";
import { AppErrorBoundary } from "./components/AppRecovery/AppRecovery";
import { BootScreen } from "rain-ui";
import { initializeFrontendHealth } from "./platform/frontendHealth";
import { installAnimationIdleThrottle } from "./platform/animationIdleThrottle";
import { installExternalLinkGuard } from "./platform/externalLinks";
import "./platform/font";
import "./styles/index.css";

const boot = new URLSearchParams(window.location.search).get("boot") === "1";
// Keep application menus, while leaving the native edit menu available for text.
document.addEventListener("contextmenu", (event) => {
  const target = event.target instanceof Element ? event.target : null;
  if (target?.closest("input, textarea, select, [contenteditable='true'], [role='textbox']")) return;
  const selection = window.getSelection();
  if (target && selection && !selection.isCollapsed && selection.anchorNode && selection.focusNode
    && target.contains(selection.anchorNode) && target.contains(selection.focusNode)) return;
  event.preventDefault();
}, { capture: true });
initializeFrontendHealth();
installAnimationIdleThrottle();
installExternalLinkGuard();
if (boot) {
  document.documentElement.classList.add("boot-mode");
}

const rootElement = document.getElementById("root");
if (rootElement) {
  ReactDOM.createRoot(rootElement).render(
    <AppErrorBoundary>{boot ? <BootScreen /> : <Startup />}</AppErrorBoundary>
  );
}
