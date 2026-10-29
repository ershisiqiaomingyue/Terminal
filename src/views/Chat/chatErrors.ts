import type { AgentEventPayload } from "@/platform/chatApi";
import type { ChatMessage } from "./types";

export function assistantErrorText(message: string): string {
  if (message.startsWith("[turn_idle_timeout]")) {
    return "长时间未收到模型或工具进度，本轮已自动停止。已保留此前输出和工具记录，请检查执行结果后继续。";
  }
  if (message.includes("Provider 流式响应在终态前断开")) {
    return "模型连接在回复完成前断开，已保留收到的正文和指令记录。为避免重复执行，未自动重试，请检查执行结果后继续。";
  }
  if (/(?:免费额度|调用额度|模型额度|可用额度).{0,32}(?:已用完|耗尽|不足)|(?:已用完|耗尽).{0,32}(?:免费额度|调用额度|模型额度|可用额度)/i.test(message)) {
    return message;
  }
  const credentialUnavailable = /\b(?:unauthorized|forbidden|invalid_api_key|invalid api key|authentication|credential|denied access|rejected credentials)\b/i.test(message)
    || /(?:API\s*Key|密钥|凭据).*(?:无效|错误|拒绝|失败)/i.test(message);
  const modelUnavailable = /\b(?:model_not_found|model_not_available|invalid_model)\b/i.test(message)
    || /\bmodel\b[^\r\n]{0,160}\b(?:not found|does not exist|doesn't exist|not available|do not have access)\b/i.test(message)
    || /(?:模型不存在|未找到模型|模型未找到|模型不可用)/.test(message);
  if (credentialUnavailable) {
    return "供应商认证或账户权限不足，模型请求尚未开始。请检查设置中的 API Key、访问令牌和供应商账户状态；这不是本地工具的“完全访问”权限问题。";
  }
  if (/\bHTTP\s*406\b/i.test(message)) {
    return "供应商拒绝了当前模型请求（HTTP 406），可能与系统提示词或请求参数的协议兼容性有关。请更新对应插件或接口适配，并运行推理测试；已保留收到的内容。";
  }
  if (/\b(?:upstream_error|overloaded_error|server_failure)\b|provider server failure|\bHTTP\s*5\d{2}\b/i.test(message)) {
    return "供应商上游模型服务暂时不可用，本次回复已中断，已保留收到的内容。请稍后重试；如持续出现，请联系供应商检查该模型线路。";
  }
  if (modelUnavailable) {
    return "当前模型不可用：供应商没有接受这个模型 ID，或当前账户无权访问它。请在设置中重新选择该供应商提供的模型，并检查 API Key；这不是本地工具的“完全访问”权限问题。";
  }
  if (/(?:未配置模型|没有可用模型|no model configured)/i.test(message)) {
    return "当前会话没有可用模型，请先在设置中保存供应商并选择模型。工具权限设置不会替代模型配置。";
  }
  return message;
}

export function assistantTerminalNotice(message: ChatMessage, processEvents: AgentEventPayload[] = []) {
  if (message.role !== "assistant" || message.pending) return null;
  const events = message.events || [];
  const findDiagnostic = (items: AgentEventPayload[]) => [...items].reverse().find((event) =>
    ((event.type === "status" && event.terminalDiagnostic === true) || event.type === "error")
    && typeof event.message === "string" && Boolean(event.message.trim()));
  // Message-owned diagnostics survive execution-detail loading and pagination.
  const diagnostic = findDiagnostic(events) || findDiagnostic(processEvents);
  const hasModelText = events.some((event) => ["snapshot_text", "text_delta"].includes(String(event.type))
    && typeof event.text === "string" && Boolean(event.text.trim()));
  // Older hosts and terminal-only snapshot projections may have no separate
  // diagnostic item. A cancellation must remain visible beside partial text.
  const cancelled = [...events].reverse().find((event) => event.type === "cancelled");
  if (!diagnostic && cancelled) return {
    text: typeof cancelled.message === "string" && cancelled.message.trim()
      ? cancelled.message : "本轮已停止，已保留此前输出和工具记录。需要继续时可发送消息。",
    failed: false,
    contentIsDiagnostic: !hasModelText && message.content.trim() === "已停止。",
  };
  if (!diagnostic && (!message.error || (hasModelText && !message.content.trim()))) return null;
  const raw = diagnostic ? String(diagnostic.message)
    : hasModelText ? "本轮执行中断，已保留收到的正文和工具记录。请检查执行结果后继续。"
    : message.content || "任务执行失败。";
  const text = assistantErrorText(raw);
  return {
    text,
    failed: diagnostic ? diagnostic.type === "error" || diagnostic.status === "failed" : true,
    contentIsDiagnostic: !hasModelText && (!diagnostic || [raw.trim(), text.trim()].includes(message.content.trim())),
  };
}
