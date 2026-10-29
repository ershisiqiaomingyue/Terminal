/**
 * 终端纯文本工具：去 ANSI / 控制字符、按行与字节裁剪、敏感信息脱敏
 * （开发方案 8.6 与 9.2.2 / S4）。
 *
 * 为什么单独成文件：这些函数既要给 block 层裁剪输出用，也要给 AI 请求体组装用，
 * 而且在 Node 单测下必须能跑（不依赖 DOM 与内核）。因此这里只做字符串运算。
 */

/** CSI 序列：ESC [ 参数 中间字节 终止字节（含 SGR、光标移动、清屏等）。 */
const CSI_PATTERN = /\u001b\[[0-9;:?<>=!]*[ -/]*[@-~]/g;
/** OSC 序列：ESC ] ... (BEL 或 ST 结束)，含 OSC 8 超链接、OSC 52 剪贴板。 */
const OSC_PATTERN = /\u001b\][^\u0007\u001b]*(?:\u0007|\u001b\\)/g;
/** 其他两字节转义（ESC 后跟单个字符，如 ESC c 复位、ESC M 反索引）。 */
const ESC_PATTERN = /\u001b[@-Z\\-_]/g;
/** DCS / SOS / PM / APC 字符串：ESC P|X|^|_ ... ST。 */
const STRING_PATTERN = /\u001b[PX^_][\s\S]*?\u001b\\/g;

/** 所有需要剔除的不可见字节；`\r` 保留由 `normalizeNewlines` 单独处理。 */
export function stripAnsi(text: string): string {
  if (!text) return "";
  return text
    .replace(STRING_PATTERN, "")
    .replace(OSC_PATTERN, "")
    .replace(CSI_PATTERN, "")
    .replace(ESC_PATTERN, "");
}

/**
 * 单行内的 `\r` 覆盖语义：每次回车回到行首，后面的内容覆盖前面的同长度部分。
 * `toPlainText` 与提示符检测（`lastOutputLine`）共用同一实现，保证两处口径一致。
 */
export function applyCarriageReturns(line: string): string {
  if (line.indexOf("\r") < 0) return line;
  let current = "";
  for (const piece of line.split("\r")) {
    current = piece.length >= current.length ? piece : piece + current.slice(piece.length);
  }
  return current;
}

/**
 * 把终端输出整理为"可读纯文本"：
 * - 去 ANSI 与控制字符；
 * - `\r` 覆盖语义：`进度 10%\r进度 20%` 只保留最后一次，符合屏幕实际显示；
 * - 去掉 NUL 与退格残留。
 */
export function toPlainText(text: string): string {
  if (!text) return "";
  const withoutAnsi = stripAnsi(text);
  const normalized = withoutAnsi.split("\n").map(applyCarriageReturns);
  return normalized.join("\n").replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/g, "");
}

/** 按 UTF-8 字节数计长；中文一个字符 3 字节，不能用 `string.length`。 */
export function utf8ByteLength(text: string): number {
  if (!text) return 0;
  let bytes = 0;
  for (const char of text) {
    const code = char.codePointAt(0) ?? 0;
    bytes += code <= 0x7f ? 1 : code <= 0x7ff ? 2 : code <= 0xffff ? 3 : 4;
  }
  return bytes;
}

export interface TruncateOptions {
  maxLines?: number;
  maxBytes?: number;
  /** 保留尾部（AI 诊断最需要最后报的错）；默认 true。 */
  keepTail?: boolean;
}

export interface TruncateResult {
  text: string;
  truncated: boolean;
}

/**
 * 按行数与 UTF-8 字节数裁剪文本。
 * 先按行裁，再按字节裁；字节只能从行首截断时也保证不切坏代理对。
 */
export function truncateText(text: string, options: TruncateOptions = {}): TruncateResult {
  const maxLines = Math.max(0, Math.floor(options.maxLines ?? Number.POSITIVE_INFINITY));
  const maxBytes = Math.max(0, Math.floor(options.maxBytes ?? Number.POSITIVE_INFINITY));
  const keepTail = options.keepTail !== false;
  if (!text) return { text: "", truncated: false };

  let truncated = false;
  let lines = text.split("\n");
  if (Number.isFinite(maxLines) && lines.length > maxLines) {
    lines = keepTail ? lines.slice(lines.length - maxLines) : lines.slice(0, maxLines);
    truncated = true;
  }

  let result = lines.join("\n");
  if (Number.isFinite(maxBytes) && utf8ByteLength(result) > maxBytes) {
    truncated = true;
    result = keepTail ? tailByBytes(result, maxBytes) : headByBytes(result, maxBytes);
  }
  return { text: result, truncated };
}

/**
 * 从尾部取不超过 maxBytes 的最长后缀。
 *
 * 必须按 **code point** 切，不能用 code unit 下标回退：`codePointAt(i-1)`
 * 在 i 落在代理对中间时读到的是**低代理**（0xDC00 段），单独算宽度会把
 * emoji 切成半个字符。这里先拆成 code point 数组再自后往前累加；
 * 裁剪是低频路径（只在超过预算时触发），这点分配可以接受。
 */
export function tailByBytes(text: string, maxBytes: number): string {
  if (maxBytes <= 0 || !text) return "";
  const points = Array.from(text);
  let bytes = 0;
  let index = points.length;
  while (index > 0) {
    const code = points[index - 1].codePointAt(0) ?? 0;
    const size = code <= 0x7f ? 1 : code <= 0x7ff ? 2 : code <= 0xffff ? 3 : 4;
    if (bytes + size > maxBytes) break;
    bytes += size;
    index -= 1;
  }
  return points.slice(index).join("");
}

/** 从头部取不超过 maxBytes 的最长前缀（同样按 code point 对齐）。 */
export function headByBytes(text: string, maxBytes: number): string {
  if (maxBytes <= 0 || !text) return "";
  let bytes = 0;
  let index = 0;
  for (const char of text) {
    const code = char.codePointAt(0) ?? 0;
    const size = code <= 0x7f ? 1 : code <= 0x7ff ? 2 : code <= 0xffff ? 3 : 4;
    if (bytes + size > maxBytes) break;
    bytes += size;
    index += char.length;
  }
  return text.slice(0, index);
}

export interface RedactOptions {
  /** 是否脱敏设备序列号；默认 true（S4）。 */
  serial?: boolean;
  /** 是否脱敏 IP 地址；默认 true。 */
  ip?: boolean;
  /** 是否脱敏家目录用户名；默认 true。 */
  homePath?: boolean;
  /** 是否脱敏 token 形态的字符串；默认 true。 */
  token?: boolean;
}

/**
 * 输出上传给模型前的脱敏（S4）。
 *
 * 处理范围保守：只动"可识别"的形态，不试图改命令语义。
 * - 序列号：保留前 4 位，其余打星；
 * - IPv4 / IPv6；
 * - `C:\Users\<name>`、`/home/<name>`、`/Users/<name>`；
 * - 常见 token 形态（`sk-...`、`ghp_...`、32 位以上十六进制串）。
 */
export function redactSensitive(text: string, options: RedactOptions = {}): string {
  if (!text) return "";
  const useSerial = options.serial !== false;
  const useIp = options.ip !== false;
  const useHome = options.homePath !== false;
  const useToken = options.token !== false;
  let result = text;

  if (useToken) {
    result = result
      .replace(/\b(sk|pk|rk)-[A-Za-z0-9_-]{16,}/g, "[token]")
      .replace(/\bgh[pousr]_[A-Za-z0-9]{20,}/g, "[token]")
      .replace(/\b[A-Fa-f0-9]{32,}\b/g, "[token]");
  }
  if (useHome) {
    result = result
      .replace(/([A-Za-z]:\\Users\\)([^\\\s"']+)/gi, "$1***")
      .replace(/(\/home\/)([^/\s"']+)/g, "$1***")
      .replace(/(\/Users\/)([^/\s"']+)/g, "$1***");
  }
  if (useIp) {
    // IPv4：只脱敏私网与公网地址段都按同一规则，避免漏掉内网拓扑泄露。
    result = result.replace(/\b(?:\d{1,3}\.){3}\d{1,3}\b/g, (match) => {
      const parts = match.split(".");
      if (parts.some((part) => Number(part) > 255)) return match;
      return `${parts[0]}.${parts[1]}.*.*`;
    });
  }
  if (useSerial) {
    // Android 序列号：无分隔符的 10 位以上字母数字串，保留前 4 位。
    // 收紧条件避免误伤普通过滤词：必须同时含字母与数字，且**至少含 3 个数字**，
    // 并且要么全为大写/数字，要么是纯十六进制——真实序列号都满足其一，
    // 而 `ContentResolver`、`uiautomator` 这类标识符不满足。
    result = result.replace(/\b([A-Za-z0-9]{4})([A-Za-z0-9]{6,})\b/g, (match, head: string) => {
      const digits = (match.match(/[0-9]/g) || []).length;
      if (!/[0-9]/.test(match) || !/[A-Za-z]/.test(match) || digits < 3) return match;
      const upperish = match === match.toUpperCase();
      const hexish = /^[0-9A-Fa-f]+$/.test(match);
      if (!upperish && !hexish) return match;
      return `${head}${"*".repeat(Math.min(8, match.length - head.length))}`;
    });
  }
  return result;
}

/** 设备序列号的展示用脱敏：保留前 4 位（9.2.2）。 */
export function redactSerial(serial: string | undefined): string | undefined {
  if (!serial) return serial;
  const trimmed = serial.trim();
  if (trimmed.length <= 4) return trimmed;
  return `${trimmed.slice(0, 4)}${"*".repeat(Math.min(6, trimmed.length - 4))}`;
}

export interface BlockTextOptions extends TruncateOptions, RedactOptions {
  /** 是否在截断处加省略标记；默认 false（AI 侧用 outputTruncated 字段告知）。 */
  marker?: string;
}

/**
 * 把 block 的输出整理成可直接进 prompt 的纯文本（8.6 / F42）。
 * 顺序：去 ANSI → 裁剪 → 脱敏；先裁剪再脱敏能保证脱敏后的体积仍可控。
 */
export function blockOutputText(raw: string, options: BlockTextOptions = {}): TruncateResult {
  const plain = toPlainText(raw);
  const { text, truncated } = truncateText(plain, options);
  const redacted = redactSensitive(text, options);
  const marker = options.marker && truncated ? options.marker : "";
  return { text: marker ? `${marker}${redacted}` : redacted, truncated };
}
