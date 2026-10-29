import type { AgentModelSettings, AgentProvider } from "./api.ts";

export type AgentProviderResponse = Omit<AgentProvider, "models"> & {
  models: Array<Omit<AgentModelSettings, "name"> & { name?: string; model?: string }>;
};

// Runtime summaries contain a qualified id and a separate wire model name.
export function normalizeAgentProviders(providers: AgentProviderResponse[]): AgentProvider[] {
  return providers.map((provider) => ({
    ...provider,
    models: provider.models.map((model) => ({
      ...model,
      id: model.model || model.id,
      name: model.name || model.model || model.id,
    })),
  }));
}
