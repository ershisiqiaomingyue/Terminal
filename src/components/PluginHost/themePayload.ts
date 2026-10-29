export type ThemeStyleDeclaration = Iterable<string> & {
  getPropertyValue: (property: string) => string;
  colorScheme?: string;
};

export type PluginThemePayload = {
  tokens: Record<string, string>;
  theme: string | undefined;
  colorScheme: string | undefined;
};

export function buildPluginThemePayload(computed: ThemeStyleDeclaration, theme: string | undefined): PluginThemePayload {
  const tokens: Record<string, string> = {};
  for (const name of computed) {
    if (name.startsWith("--")) tokens[name] = computed.getPropertyValue(name);
  }
  return { tokens, theme, colorScheme: computed.colorScheme };
}
