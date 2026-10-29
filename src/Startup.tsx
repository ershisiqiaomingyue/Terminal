import { StrictMode, useEffect, useRef, useState, type ReactNode } from "react";
import { AppRecovery } from "./components/AppRecovery/AppRecovery";
import { initializeProjectPreferences } from "./platform/projectPreferences";
import { initializeToolWindow, isToolWindow } from "./platform/toolWindows";
import { initializeThemeSync } from "./platform/theme";
import { startupStep } from "./platform/startupStep";
import { reportDiagnostic, reportFrontendError } from "./platform/frontendHealth";
import { recoverInterruptedAgentRuns } from "./platform/chatApi";
import { invoke, isTauri, formatInvokeError } from "./platform/runtime";
import { isNoticeWindow } from "./platform/noticeWindow";

export function Startup() {
  const startedAt = useRef(performance.now());
  // 提示窗是个一次性的小窗口：不碰历史库、不调 finish_boot，只同步主题让配色对上。
  const notice = isNoticeWindow();
  const [content, setContent] = useState<ReactNode>(null);
  const [stage, setStage] = useState("正在恢复应用配置与会话…");
  const [error, setError] = useState("");
  // 启动不做迁移门控：直接进入界面。

  const upgradeSettled = true;
  // 弹过窗就得等用户点，finally 里的放行只对「不需要弹窗」生效。
  useEffect(() => {
    let active = true;
    const start = async () => {
      if (notice) {
        await startupStep("主题初始化", initializeThemeSync).catch(() => undefined);
        if (active) {
          const { NoticeWindow } = await import("./NoticeWindow");
          setContent(<StrictMode><NoticeWindow /></StrictMode>);
        }
        return;
      }
      if (!upgradeSettled) return;
      // Start database repair immediately, alongside configuration work. No
      // conversation view or read-ahead cache may load before it commits.
      const history = isTauri() && !isToolWindow()
        ? (async () => {
          await startupStep("会话状态恢复", recoverInterruptedAgentRuns);
          // 预加载只是暖缓存：历史库很大时（几亿字节的 agent-history）读整段
          // 消息会超过启动预算。超时/失败都只记一条诊断，绝不能让应用起不来——
          // 聊天视图挂载后会自己按需读取，读到的内容一致。
          await startupStep("会话预加载", async () => {
            const { warmConversationHistory } = await import("./views/Chat/preloadConversation");
            await warmConversationHistory();
          }).catch((reason: unknown) => {
            reportDiagnostic("conversation_preload_skipped");
            console.warn("会话预加载已跳过，改为进入界面后按需读取", reason);
          });
        })()
        : Promise.resolve();
      const configuration = async () => {
        await startupStep("主题初始化", initializeThemeSync);
        await startupStep("项目配置恢复", initializeProjectPreferences);
        return startupStep("工具窗口恢复", initializeToolWindow);
      };
      const [info] = await Promise.all([configuration(), history]);
      if (active) setStage("正在加载界面…");
      const view = info
        ? await startupStep("工具界面加载", async () => {
          const { ToolWindowApp } = await import("./ToolWindowApp");
          return <ToolWindowApp info={info} />;
        })
        : await startupStep("主界面加载", async () => {
          const { default: App } = await import("./App");
          return <App />;
        }, 30_000);
      if (active) { setContent(<StrictMode>{view}</StrictMode>); reportDiagnostic("startup_ready"); }
    };
    void start().catch((reason: unknown) => {
      console.error("应用启动失败", reason);
      reportFrontendError("startup_failed", reason);
      if (active) setError(formatInvokeError(reason));
    });
    return () => { active = false; };
  }, [upgradeSettled]);
  // 本项目不做「升级 / 数据迁移」欢迎窗：启动即进入界面。
  useEffect(() => {
    if (notice) return;
    if ((!content && !error) || !isTauri() || isToolWindow() || !upgradeSettled) return;
    // The 900ms animation runs concurrently with preload, never before it.
    const timer = window.setTimeout(() => {
      void invoke("finish_boot", { startupReady: !error }).catch((reason: unknown) => {
        reportFrontendError("startup_window_failed", reason);
      });
    }, Math.max(0, 900 - (performance.now() - startedAt.current)));
    return () => window.clearTimeout(timer);
  }, [content, error, upgradeSettled]);
  return content || <AppRecovery title={error ? "应用启动失败" : "正在启动 Terminal"} message={error || stage} loading={!error} />;
}
