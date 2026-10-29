import hljs from "highlight.js/lib/core";
import awk from "highlight.js/lib/languages/awk";
import bash from "highlight.js/lib/languages/bash";
import cmake from "highlight.js/lib/languages/cmake";
import cpp from "highlight.js/lib/languages/cpp";
import csharp from "highlight.js/lib/languages/csharp";
import css from "highlight.js/lib/languages/css";
import dart from "highlight.js/lib/languages/dart";
import diff from "highlight.js/lib/languages/diff";
import dockerfile from "highlight.js/lib/languages/dockerfile";
import dos from "highlight.js/lib/languages/dos";
import go from "highlight.js/lib/languages/go";
import gradle from "highlight.js/lib/languages/gradle";
import groovy from "highlight.js/lib/languages/groovy";
import ini from "highlight.js/lib/languages/ini";
import java from "highlight.js/lib/languages/java";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import kotlin from "highlight.js/lib/languages/kotlin";
import lua from "highlight.js/lib/languages/lua";
import makefile from "highlight.js/lib/languages/makefile";
import markdown from "highlight.js/lib/languages/markdown";
import perl from "highlight.js/lib/languages/perl";
import php from "highlight.js/lib/languages/php";
import powershell from "highlight.js/lib/languages/powershell";
import properties from "highlight.js/lib/languages/properties";
import protobuf from "highlight.js/lib/languages/protobuf";
import python from "highlight.js/lib/languages/python";
import ruby from "highlight.js/lib/languages/ruby";
import rust from "highlight.js/lib/languages/rust";
import shell from "highlight.js/lib/languages/shell";
import sql from "highlight.js/lib/languages/sql";
import swift from "highlight.js/lib/languages/swift";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";

export const MAX_HIGHLIGHT_BYTES = 256 * 1024;

for (const [name, language] of Object.entries({ awk, bash, cmake, cpp, csharp, css, dart, diff, dockerfile, dos, go, gradle, groovy, ini, java, javascript, json, kotlin, lua, makefile, markdown, perl, php, powershell, properties, protobuf, python, ruby, rust, shell, sql, swift, typescript, xml, yaml })) {
  hljs.registerLanguage(name, language);
}

export function escapeHtml(source: string) {
  return source.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

// 扩展名 → highlight.js 语言名；既有条目保持原样，仅追加新语言
const EXTENSION_LANGUAGES: Record<string, string> = {
  ts: "typescript", tsx: "typescript", js: "javascript", jsx: "javascript", rs: "rust", java: "java", kt: "kotlin", kts: "kotlin",
  xml: "xml", html: "xml", json: "json", md: "markdown", markdown: "markdown", css: "css", scss: "css", h: "cpp", hpp: "cpp",
  c: "cpp", cc: "cpp", cpp: "cpp", gradle: "gradle", py: "python", sh: "bash", bash: "bash", zsh: "bash", ps1: "powershell",
  yml: "yaml", yaml: "yaml",
  go: "go", cs: "csharp", csx: "csharp", php: "php", rb: "ruby", rake: "ruby", gemspec: "ruby", swift: "swift", dart: "dart",
  lua: "lua", sql: "sql", toml: "ini", ini: "ini", cfg: "ini", dockerfile: "dockerfile",
  // Windows 批处理与常见构建/配置脚本
  bat: "dos", cmd: "dos", mk: "makefile", cmake: "cmake", properties: "properties", env: "properties",
  proto: "protobuf", groovy: "groovy", gvy: "groovy", diff: "diff", patch: "diff", pl: "perl", pm: "perl", awk: "awk",
  // Shell 家族的其他后缀
  fish: "bash", ksh: "bash", command: "bash",
};

// 无扩展名的特殊文件名：整名匹配（Dockerfile、Dockerfile.dev、dir/Dockerfile …）
const FILE_NAME_LANGUAGES: Record<string, string> = {
  dockerfile: "dockerfile",
  makefile: "makefile", gnumakefile: "makefile",
  cmakelists: "cmake",
  // 没有扩展名的 Shell 配置与启动脚本
  ".bashrc": "bash", ".bash_profile": "bash", ".bash_aliases": "bash", ".bash_login": "bash", ".bash_logout": "bash",
  ".zshrc": "bash", ".zshenv": "bash", ".zprofile": "bash", ".zlogin": "bash",
  ".profile": "bash", ".shrc": "bash",
  // 常见属性/环境配置
  ".editorconfig": "properties", ".env": "properties", ".gitattributes": "ini", ".gitconfig": "ini", ".npmrc": "properties",
};

// 先按文件名整体匹配（大小写不敏感、忽略目录），再回落到扩展名
export function resolveLanguage(path: string): string | undefined {
  const fileName = path.split(/[\\/]/).pop()?.toLowerCase() || "";
  for (const [name, language] of Object.entries(FILE_NAME_LANGUAGES)) {
    if (fileName === name || fileName.startsWith(`${name}.`)) return language;
  }
  const ext = path.split(".").pop()?.toLowerCase() || "";
  return EXTENSION_LANGUAGES[ext] as string | undefined;
}

export function highlightSource(source: string, path: string) {
  const language = resolveLanguage(path);
  if (!language || source.length > MAX_HIGHLIGHT_BYTES) return escapeHtml(source);
  try { return hljs.highlight(source, { language, ignoreIllegals: true }).value; }
  catch { return escapeHtml(source); }
}
