export type LogLineClass = "log-error" | "log-warning" | "log-info" | "";

// Keep patterns outside rendering and avoid stateful /g expressions. Word
// boundaries prevent matches inside words such as information or errorprone.
const ERROR = /\b(?:err(?:or)?s?|excep(?:tion)?s?|fatal|fail(?:ed|ure)?)\b|\[(?:错误|异常|失败)\]/i;
const EXCEPTION_TYPE = /\b[A-Za-z_$][\w$]*(?:Exception|Error)\b/;
const WARNING = /\bwarn(?:ings?)?\b|\[警告\]/i;
const INFO = /\b(?:info|success(?:ful(?:ly)?)?)\b|\[(?:信息|成功)\]/i;

/** Classify plain log text; severity takes priority over informational labels. */
export function logLineClass(line: string): LogLineClass {
  if (ERROR.test(line) || EXCEPTION_TYPE.test(line)) return "log-error";
  if (WARNING.test(line)) return "log-warning";
  if (INFO.test(line)) return "log-info";
  return "";
}
