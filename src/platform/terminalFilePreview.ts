/**
 * TerminalFilePreview
 * ---------------------------------------------------------------------------
 * 终端右栏「文件浏览」的打开前判定（v2 设计文档 5.2 / 5.3）。
 *
 * 这里只放**纯函数**，不发任何请求：
 *   ① 5.3 的判定顺序完全靠目录项 name/size 就能算出来，做成纯函数才能用
 *      `node --experimental-strip-types --test` 直接跑（本仓 tests/*.test.mts 的做法）。
 *   ② IO 仍留在 `src/platform/terminalWorkspace.ts`（listSshDirectory /
 *      listLocalDirectory / readRemoteTextFile），
 *      避免为了一次预览判定把两套通路再抄一遍。
 */

/** 单个文件的预览上限。与后端硬约束保持一致：
 *  `code_viewer.rs` 的 MAX_FILE_BYTES = 1 MB。 */
export const PREVIEW_LIMITS = { local: 1024 * 1024, ssh: 1024 * 1024 } as const;

export type PreviewSourceKind = keyof typeof PREVIEW_LIMITS;

/** 预览来源：由焦点页签推导（local / ssh 两条通路）。 */
export interface PreviewSource {
  kind: PreviewSourceKind;
  /** 该来源可浏览的根目录；本地/SSH 为会话 root。 */
  root: string;
}

/** 目录项（与 DirectoryEntry 同构，取其最小子集）。 */
export interface PreviewEntryLike {
  name: string;
  path: string;
  isDirectory: boolean;
  /** 目录项的大小可能未知；列表接口给的是 `number | null`。 */
  size?: number | null;
}

export type PreviewInspection =
  | { ok: true }
  | {
      ok: false;
      reason: "too-large" | "unsupported" | "no-root";
      message: string;
      /** 仅 reason === "too-large" 时给出，供界面显示「上限 N MB」。 */
      limit?: number;
    };

/** 二进制/归档/媒体：后端 is_non_editable_file 已经拒，前端提前拦下省一次往返。 */
export const BINARY_EXT = new Set([
  "apk", "aar", "jar", "so", "dex", "class", "zip", "7z", "rar", "gz", "xz",
  "png", "jpg", "jpeg", "webp", "gif", "ico", "woff", "woff2", "ttf", "otf",
  "mp3", "wav", "mp4", "avi", "pdf", "bin", "exe", "dll", "dylib", "a", "o", "wasm",
]);

/**
 * 明确是文本的扩展名。
 * 注意它不是「白名单」——不在这里且大小未知的文件仍然允许请求（5.3 第 5 条），
 * 由后端 UTF-8 校验兜底；否则 `Makefile`、`Dockerfile` 这类无扩展名文件会被误杀。
 */
export const TEXT_HINT_EXT = new Set([
  "txt", "log", "json", "md", "xml", "yaml", "yml", "ini", "conf", "properties", "gradle", "sh", "bat", "ps1",
  "ts", "tsx", "js", "jsx", "rs", "kt", "java", "c", "h", "cpp", "py", "go", "css", "html", "sql", "toml", "diff", "patch",
]);

/** 取小写扩展名（不含点）；无扩展名返回空串。 */
export function previewExt(name: string): string {
  const dot = name.lastIndexOf(".");
  // 以点开头的隐藏文件（.bashrc）按「无扩展名」处理，否则 ext 会变成整个文件名。
  if (dot <= 0 || dot === name.length - 1) return "";
  return name.slice(dot + 1).toLowerCase();
}

/** 当前来源的单文件预览上限。 */
export function previewLimit(source: PreviewSource): number {
  return PREVIEW_LIMITS[source.kind];
}

/** 人类可读的 MB 文案，用于提示语（1 MB / 8 MB）。 */
export function formatPreviewLimit(limit: number): string {
  const mb = limit / (1024 * 1024);
  return `${Number.isInteger(mb) ? mb : mb.toFixed(1)} MB`;
}

/**
 * 打开前的廉价判定（5.3 判定顺序，逐条对齐文档）：
 *   1. 目录 → 下钻，不判
 *   2. 无 root → no-root
 *   3. 二进制扩展名 → unsupported
 *   4. 已知大小超限 → too-large
 *   5. 其余 → ok（大小未知也放行，交给后端兜底）
 */
export function inspectPreviewEntry(source: PreviewSource, entry: PreviewEntryLike): PreviewInspection {
  if (entry.isDirectory) return { ok: true };

  if (!source.root) {
    return { ok: false, reason: "no-root", message: "该会话没有可用的根目录" };
  }

  const ext = previewExt(entry.name);
  if (ext && BINARY_EXT.has(ext)) {
    return {
      ok: false,
      reason: "unsupported",
      message: `该类型文件不支持在终端内预览（.${ext}）`,
    };
  }

  const limit = previewLimit(source);
  if (typeof entry.size === "number" && entry.size > limit) {
    return {
      ok: false,
      reason: "too-large",
      message: `文件 ${formatPreviewLimit(entry.size)}，超过 ${formatPreviewLimit(limit)} 上限，无法在终端内预览`,
      limit,
    };
  }

  return { ok: true };
}
