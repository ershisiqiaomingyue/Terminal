import { useEffect, useMemo, useRef, useState } from "react";
import {
  AlertTriangle,
  ArrowRight,
  Check,
  CircleDot,
  DatabaseBackup,
  FolderCog,
  Sparkles,
  Wrench,
} from "lucide-react";
import { Button } from "@/components/ui/Button";
import { Spinner } from "@/components/ui/Spinner";
import { reportFrontendError } from "@/platform/frontendHealth";
import {
  acknowledgeUpgrade,
  readUpgradeStatus,
  requiredSteps,
  runUpgrade,
  type UpgradeStatus,
  type UpgradeStep,
} from "@/platform/upgrade";
import "./upgrade-welcome.css";

/** 每个迁移项配一个图标；后端新增未知 id 时退化成通用图标。 */
const STEP_ICONS: Record<string, typeof DatabaseBackup> = {
  "user-storage": DatabaseBackup,
  "environment-tools": Wrench,
  "workspace-config": FolderCog,
};

type Phase = "loading" | "ready" | "running" | "done" | "error";

/**
 * 2.0 大升级的欢迎窗。它出现在主界面之前，挡住所有其它视图——因为任何界面
 * 都会去读数据库、写配置，而那时数据可能还在旧路径里。
 *
 * 三种状态：
 * 1. 有旧数据没搬：列出清单 + 「开始迁移」，搬完之前按钮不可关闭。
 * 2. 搬完了 / 全新安装：只欢迎，点「开始使用」写一条已确认记录。
 * 3. 失败：原样展示错误并允许重试，绝不静默放行。
 */
export function UpgradeWelcome({ onDone }: { onDone: () => void }) {
  const [phase, setPhase] = useState<Phase>("loading");
  const [status, setStatus] = useState<UpgradeStatus | null>(null);
  const [error, setError] = useState("");
  const started = useRef(false);

  useEffect(() => {
    let active = true;
    readUpgradeStatus()
      .then((next) => {
        if (!active) return;
        setStatus(next);
        setPhase("ready");
      })
      .catch((reason: unknown) => {
        if (!active) return;
        setError(reason instanceof Error ? reason.message : String(reason));
        setPhase("error");
      });
    return () => { active = false; };
  }, []);

  const steps = status?.steps ?? [];
  const required = useMemo(() => (status ? requiredSteps(status) : []), [status]);
  const needsMigration = required.length > 0;
  const migrated = steps.filter((step) => step.status === "done").length;

  const migrate = async () => {
    if (started.current) return;
    started.current = true;
    setPhase("running");
    setError("");
    try {
      const result = await runUpgrade();
      setStatus((previous) => previous && { ...previous, steps: result.steps, pending: result.pending, error: result.error });
      if (result.error) {
        setError(result.error);
        setPhase("error");
        return;
      }
      setPhase("done");
    } catch (reason: unknown) {
      reportFrontendError("upgrade_failed", reason);
      setError(reason instanceof Error ? reason.message : String(reason));
      setPhase("error");
    } finally {
      started.current = false;
    }
  };

  const finish = async () => {
    // 记录失败也不能把用户困在欢迎窗里：下次启动会再问一次，但至少这次能进。
    await acknowledgeUpgrade().catch((reason: unknown) => console.warn("升级状态保存失败", reason));
    onDone();
  };

  const busy = phase === "loading" || phase === "running";

  return (
    <main className="upgrade-welcome" role="dialog" aria-modal="true" aria-labelledby="upgrade-welcome-title">
      <section className="upgrade-welcome-card">
        <header className="upgrade-welcome-head">
          <span className="upgrade-welcome-badge" aria-hidden="true">
            <Sparkles size={18} />
          </span>
          <div>
            <p className="upgrade-welcome-kicker">v2.0 · Android Toolkit</p>
            <h1 id="upgrade-welcome-title">
              {needsMigration && phase !== "done" ? "新版本需要先迁移一次数据" : "欢迎使用 Android Toolkit 2.0"}
            </h1>
            <p className="upgrade-welcome-lead">
              {needsMigration && phase !== "done"
                ? "产品从 Android Tool 升级为 Android Toolkit，用户目录与项目里的配置文件名也跟着换了。下面这些内容会搬到新位置，旧文件原样保留，随时可以回退。"
                : "2.0 已完成数据迁移：历史对话、模型接入、环境工具与项目配置都在新位置，直接开始使用即可。"}
            </p>
          </div>
        </header>

        <ul className="upgrade-welcome-steps">
          {phase === "loading" && <li className="upgrade-welcome-loading"><Spinner size={14} /> 正在检查旧版本数据…</li>}
          {steps.map((step) => <UpgradeStepRow key={step.id} step={step} running={phase === "running"} />)}
        </ul>

        {error && (
          <p className="upgrade-welcome-error" role="alert">
            <AlertTriangle size={14} /> {error}
          </p>
        )}

        <footer className="upgrade-welcome-foot">
          <p className="upgrade-welcome-hint">
            {phase === "done" || !needsMigration
              ? migrated > 0 ? `${migrated} 项内容已就位。` : "无需迁移，直接进入即可。"
              : "迁移过程中请勿关闭窗口。"}
          </p>
          <div className="upgrade-welcome-actions">
            {needsMigration && phase !== "done" ? (
              <Button
                variant="primary"
                size="md"
                loading={phase === "running"}
                disabled={busy && phase !== "running"}
                icon={phase === "running" ? undefined : <Sparkles size={14} />}
                onClick={() => void migrate()}
              >
                {phase === "running" ? "正在迁移…" : error ? "重试迁移" : "开始迁移"}
              </Button>
            ) : (
              <Button variant="primary" size="md" icon={<ArrowRight size={14} />} onClick={() => void finish()}>
                开始使用
              </Button>
            )}
          </div>
        </footer>
      </section>
    </main>
  );
}

function UpgradeStepRow({ step, running }: { step: UpgradeStep; running: boolean }) {
  const Icon = STEP_ICONS[step.id] ?? CircleDot;
  const state = running && step.status === "pending" ? "running" : step.status;
  const label = { pending: "待迁移", running: "迁移中", done: "已迁移", failed: "失败", missing: "无需迁移" }[state];
  return (
    <li className={`upgrade-welcome-step is-${state}`}>
      <span className="upgrade-welcome-step-icon" aria-hidden="true">
        {state === "done" ? <Check size={13} /> : state === "running" ? <Spinner size={13} /> : <Icon size={13} />}
      </span>
      <div className="upgrade-welcome-step-body">
        <b>{step.title}</b>
        <small>{step.detail}</small>
      </div>
      <span className="upgrade-welcome-step-state">{label}</span>
    </li>
  );
}

export default UpgradeWelcome;
