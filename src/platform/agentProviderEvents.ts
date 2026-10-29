import { isTauri, listen } from "./runtime.ts";

export const AGENT_PROVIDERS_CHANGED_EVENT = "android-tool-agent-providers-changed";
let bridgeStarted = false;
export function ensureAgentProviderEventsBridged() {
  if (bridgeStarted || !isTauri()) return;
  bridgeStarted = true;
  // One native listener per WebView, shared by model selectors and settings.
  void listen("model-plugins-changed", notifyAgentProvidersChanged).catch(() => { bridgeStarted = false; });
}

export function notifyAgentProvidersChanged() {
  window.dispatchEvent(new Event(AGENT_PROVIDERS_CHANGED_EVENT));
}
