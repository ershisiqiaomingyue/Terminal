type RevisionedThreadSnapshot = { id: string; revision: number };

export function shouldApplyThreadSnapshot<T extends RevisionedThreadSnapshot>(
  current: T | null,
  incoming: T,
) {
  return incoming.id !== current?.id || incoming.revision > current.revision;
}
