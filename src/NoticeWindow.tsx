import { useCallback, useEffect, useRef, useState } from "react";
import { invoke, isTauri } from "@/platform/runtime";
import "./notice-window.css";

/** 后端 CompletionNoticeInfo 的镜像（见 src-tauri/src/agent/completion_notice.rs）。 */
export interface CompletionNoticeInfo {
  label: string;
  sessionId: string;
  title: string;
  summary: string;
}

/** 提示窗停留时长：够读完两行，又不会挡着人做事。 */
const AUTO_DISMISS_MS = 7_000;

/** 浏览器里没有后端：用查询参数把窗口内容造出来，方便单独调这块 UI。 */
function previewInfo(): CompletionNoticeInfo {
  const query = new URLSearchParams(window.location.search);
  return {
    label: "preview",
    sessionId: query.get("sessionId") || "",
    title: query.get("title") || "将代码迁移至单一文件夹",
    summary: query.get("summary")
      || "已把散落在根目录的 TS 文件收进 src/，并更新了 12 处相对引用；构建与类型检查均通过，剩下 3 个测试用例待补。",
  };
}

/**
 * 会话完成的右下角提示窗。
 *
 * 排版按需求固定三块：首行「xxx 完成」，下面两行会话摘要，点击整块打开会话。
 * 这个窗口是自绘的（不是系统 toast），所以外观完全跟主界面主题走。
 */
export function NoticeWindow() {
  const [info, setInfo] = useState<CompletionNoticeInfo | null>(null);
  const [visible, setVisible] = useState(false);
  const [error, setError] = useState("");
  const closing = useRef(false);

  const dismiss = useCallback(() => {
    if (closing.current) return;
    closing.current = true;
    void invoke("dismiss_completion_notice").catch(() => undefined);
  }, []);

  const open = useCallback(() => {
    if (closing.current) return;
    closing.current = true;
    void invoke("open_completion_notice").catch(() => undefined);
  }, []);

  useEffect(() => {
    let active = true;
    if (!isTauri()) {
      // 预览模式：直接渲染，不排自动关闭（截图/调试时需要它一直在）。
      setInfo(previewInfo());
      setVisible(true);
      return () => { active = false; };
    }
    void invoke<CompletionNoticeInfo>("get_completion_notice")
      .then((data) => {
        if (!active) return;
        setInfo(data);
        // 首帧内容就位后才让后端把窗口挪到右下角并显示，避免空白闪一下。
        return invoke("show_completion_notice").then(() => {
          if (!active) return;
          setVisible(true);
        });
      })
      .catch((reason: unknown) => {
        if (!active) return;
        setError(reason instanceof Error ? reason.message : String(reason));
      });
    return () => { active = false; };
  }, []);

  useEffect(() => {
    if (!visible) return;
    const timer = window.setTimeout(dismiss, AUTO_DISMISS_MS);
    return () => window.clearTimeout(timer);
  }, [visible, dismiss]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") dismiss();
      if (event.key === "Enter") open();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [dismiss, open]);

  if (error) return null;

  return <div
    className="notice-card"
    data-visible={visible || undefined}
    role="button"
    tabIndex={0}
    aria-label={info ? `${info.title} 完成` : "会话完成"}
    onClick={open}
    onKeyDown={(event) => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); open(); } }}
  >
    <div className="notice-head">
      <span className="notice-dot" aria-hidden="true" />
      <span className="notice-title">{info?.title || "新会话"}&nbsp;完成</span>
      <button
        type="button"
        className="notice-close"
        aria-label="关闭"
        onClick={(event) => { event.stopPropagation(); dismiss(); }}
      >
        <svg viewBox="0 0 16 16" width="12" height="12" fill="none" aria-hidden="true">
          <path d="M4 4l8 8M12 4l-8 8" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" />
        </svg>
      </button>
    </div>
    <p className="notice-summary">{info?.summary || "本轮已完成，点击查看。"}</p>
  </div>;
}
