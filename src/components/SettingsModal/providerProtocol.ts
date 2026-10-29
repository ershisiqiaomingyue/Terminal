import type { AgentModelSettings } from "../../platform/api.ts";

export function modelsForProviderProtocol(models: AgentModelSettings[], protocol: string): AgentModelSettings[] {
  return models.map((model) => ({ ...model, protocol, protocols: [protocol] }));
}
