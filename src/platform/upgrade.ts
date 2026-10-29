/** 2.0 大升级：欢迎窗显示前先问后端「这一代迁移做完没有」。 */
import { invoke } from "./runtime";

export type UpgradeStepStatus = "done" | "pending" | "missing" | "failed";

export interface UpgradeStep {
  id: string;
  title: string;
  detail: string;
  status: UpgradeStepStatus;
}

export interface UpgradeStatus {
  generation: string;
  product: string;
  /** 已经点过「开始使用」：不再弹欢迎窗。 */
  acknowledged: boolean;
  steps: UpgradeStep[];
  pending: number;
  error: string;
}

export interface UpgradeRunResult {
  migrated: number;
  pending: number;
  error: string;
  steps: UpgradeStep[];
}

export function readUpgradeStatus(): Promise<UpgradeStatus> {
  return invoke<UpgradeStatus>("upgrade_status");
}

export function runUpgrade(): Promise<UpgradeRunResult> {
  return invoke<UpgradeRunResult>("run_upgrade");
}

export function acknowledgeUpgrade(): Promise<void> {
  return invoke<void>("acknowledge_upgrade");
}

/** 需要用户停下来处理的步骤（欢迎窗里唯一会显示为“待迁移”的那一类）。 */
export function requiredSteps(status: UpgradeStatus): UpgradeStep[] {
  return status.steps.filter((step) => step.status === "pending" || step.status === "failed");
}
