export type ToolEnvironment = "windows" | "ssh" | "wsl";

const TOOL_ENVIRONMENT_LABELS: Record<ToolEnvironment, string> = {
  windows: "Windows",
  ssh: "SSH",
  wsl: "WSL",
};

export function canonicalToolName(value: string) {
  const match = /^(windows|ssh|wsl)__(.+)$/i.exec(value.trim());
  return match?.[2] || value.trim();
}

export function toolEnvironment(value: string): ToolEnvironment | null {
  const match = /^(windows|ssh|wsl)__/.exec(value.trim().toLowerCase());
  return match ? match[1] as ToolEnvironment : null;
}

export function toolEnvironmentLabel(value: string) {
  const environment = toolEnvironment(value);
  return environment ? TOOL_ENVIRONMENT_LABELS[environment] : "";
}
