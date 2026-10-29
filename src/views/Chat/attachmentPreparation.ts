import type { AgentArtifact, AgentImageAttachment } from "../../platform/chatApi.ts";
import type { ChatSource } from "./types.ts";

export interface PreparedChatAttachment {
  artifact: AgentArtifact;
  image?: AgentImageAttachment | null;
  sourceVersion: string;
}

type Operations = {
  local: (path: string, previous?: PreparedChatAttachment) => Promise<PreparedChatAttachment>;
  browser: (name: string, dataUrl: string) => Promise<AgentArtifact>;
};

/** Shares attachment work between selection and send. Only in-flight inputs
 * live here; completed results belong to the draft and survive a restart. */
export function createAttachmentPreparation(operations: Operations) {
  const pending = new Map<string, { input?: string; result: Promise<PreparedChatAttachment> }>();
  const warm = (source: ChatSource): Promise<PreparedChatAttachment> => {
    if (source.prepared) return Promise.resolve(source.prepared);
    const current = pending.get(source.path);
    if (current && current.input === source.dataUrl) return current.result;
    const result = source.dataUrl
      ? operations.browser(source.name, source.dataUrl).then((artifact) => {
          if (!artifact.path) throw new Error(`附件“${source.name}”未能保存，请重试。`);
          return { artifact: { ...artifact, status: "user_attachment" }, sourceVersion: "",
            image: source.kind === "image" ? { name: source.name,
              mediaType: source.dataUrl!.match(/^data:([^;,]+)/)?.[1] || "image/png",
              dataUrl: "", path: artifact.path } : null };
        })
      : operations.local(source.path);
    const job = { input: source.dataUrl, result };
    pending.set(source.path, job);
    const release = () => { if (pending.get(source.path) === job) pending.delete(source.path); };
    void result.then(release, release);
    return result;
  };
  return {
    warm,
    async submission(sources: ChatSource[]) {
      const prepared = await Promise.all(sources.map(async (source) => {
        const ready = await warm(source);
        // Browser uploads are immutable; local selections get a cheap stat to
        // refresh changed/deleted files and repair missing cached snapshots.
        return source.path.startsWith("browser:") || Boolean(source.dataUrl)
          ? ready : operations.local(source.path, ready);
      }));
      return {
        localArtifacts: prepared.map((item) => item.artifact),
        browserArtifacts: [] as AgentArtifact[],
        imageAttachments: prepared.flatMap((item) => item.image ? [item.image] : []),
        prepared,
      };
    },
  };
}

export function preparedSource(source: ChatSource, prepared: PreparedChatAttachment): ChatSource {
  return { ...source, dataUrl: undefined, prepared, preparing: false, preparationError: undefined,
    imagePath: prepared.image?.path || source.imagePath,
    kind: prepared.artifact.kind === "directory" ? "directory" : source.kind,
    size: prepared.artifact.size ?? source.size };
}
