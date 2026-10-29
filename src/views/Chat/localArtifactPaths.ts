type LocalArtifactSource = {
  path: string;
  dataUrl?: string;
};

export function localArtifactPathKey(path: string) {
  const normalized = path.replace(/\\/g, "/").replace(/\/+$/, "");
  return /^[A-Za-z]:\//.test(normalized) || normalized.startsWith("//") ? normalized.toLowerCase() : normalized;
}

export function markLocalArtifactSources<T extends { path?: unknown; kind?: unknown; status?: unknown }>(
  artifacts: T[], workspacePath: string | undefined, sources: LocalArtifactSource[],
): T[] {
  const explicit = new Set(sources.filter((source) => !source.dataUrl).map((source) => localArtifactPathKey(source.path)));
  const workspace = workspacePath ? localArtifactPathKey(workspacePath) : undefined;
  return artifacts.map((artifact) => {
    if (artifact.kind !== "directory" || typeof artifact.path !== "string") return artifact;
    const path = localArtifactPathKey(artifact.path);
    if (explicit.has(path)) return { ...artifact, status: "user_attachment" };
    return workspace && path === workspace ? { ...artifact, status: "workspace_context" } : artifact;
  });
}

export function collectLocalArtifactPaths(
  workspacePath: string | null | undefined,
  sources: LocalArtifactSource[],
) {
  const paths = [
    workspacePath || "",
    ...sources.filter((source) => !source.dataUrl).map((source) => source.path),
  ];
  return paths.filter((path, index, all) => Boolean(path) && all.indexOf(path) === index);
}
