import type { AgentProvider } from "../../platform/api.ts";
import type { ChatUsage } from "./types.ts";

export function reconcileModelSelection(providers: AgentProvider[], selection: { providerId: string; modelId: string }) {
  const provider = providers.find((item) => item.id === selection.providerId) || providers[0];
  const models = provider?.models.length ? provider.models : provider?.model ? [{ id: provider.model }] : [];
  const savedModelId = provider && selection.modelId.startsWith(`${provider.id}:`)
    ? selection.modelId.slice(provider.id.length + 1) : selection.modelId;
  const model = models.find((item) => item.id === selection.modelId || item.id === savedModelId)
    || models.find((item) => item.id === provider?.model) || models[0];
  return { providerId: provider?.id || "", modelId: model?.id || "" };
}

// Selection changes invalidate requests even when the user switches A -> B -> A.
export class ModelRequestGuard {
  private key = "";
  private revision = 0;

  select(key: string) {
    if (this.key !== key) {
      this.key = key;
      this.revision += 1;
    }
  }

  begin() { return ++this.revision; }
  isCurrent(revision: number) { return revision === this.revision; }
  invalidate() { this.revision += 1; }
}

export function contextUsageForDisplay(running: boolean, live: ChatUsage | undefined, selectedWindow: number | null | undefined, estimatedTokens: number) {
  // The provider-reported reading is the only measured value. A stopped or
  // finished turn keeps showing its last report instead of dropping back to the
  // character estimate, which used to make the ring jump to a made-up share.
  // The window follows the selected model while idle, and the reporting model
  // while a turn is running, so a reading is never mixed with another window.
  return {
    window: running ? live?.contextWindow ?? selectedWindow : selectedWindow ?? live?.contextWindow,
    tokens: live?.contextTokens ?? estimatedTokens,
  };
}
