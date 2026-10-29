export type AndroidPackageInputKind = "archive" | "native-library";

export type AndroidPackageInput = {
  kind: AndroidPackageInputKind;
  format: string;
};

// Browsers can keep the original extension and append an ordinal, such as
// `library.aar1` or `app.apk (2)`. Keep this aligned with the Rust pattern.
const ANDROID_PACKAGE_FILE_NAME_PATTERN = /\.(apk|aar|jar|aab|apks|zip|so)(?:\d+|\.\d+|\s*\(\d+\))?$/i;

export function classifyAndroidPackageInput(path: string): AndroidPackageInput | null {
  const fileName = path.trim().split(/[\\/]/).filter(Boolean).pop() || "";
  const match = fileName.match(ANDROID_PACKAGE_FILE_NAME_PATTERN);
  if (!match) return null;

  const suffix = match[1].toLocaleLowerCase("en-US");
  return suffix === "so"
    ? { kind: "native-library", format: "ELF SO" }
    : { kind: "archive", format: suffix.toUpperCase() };
}

export function androidPackageInputHint(path: string) {
  const input = classifyAndroidPackageInput(path);
  if (input?.kind === "native-library") return "已识别原生 .so 库，将读取 ELF 文件头和架构信息。";
  if (input) return `已识别 ${input.format} 包，将读取包结构和校验信息。`;
  return "仅支持 APK、AAR、JAR、AAB、APKS、ZIP 或 SO 文件。";
}
