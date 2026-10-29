import React from "react";
import { RefreshCw } from "lucide-react";
import { Button } from "./Button";

/**
 * UsageMetrics
 * ---------------------------------------------------------------------------
 * The one-line metrics strip at the bottom of the composer: output speed, input
 * and output tokens, cache hit rate, account balance and this turn's cost.
 *
 * Shape taken from ChatUsageSummary + ChatBillingSummary: a single row above a
 * hairline divider, 10px type, each item a label plus a bold value. Balance is
 * a button because it re-reads the account.
 *
 * The caller owns formatting (token shortening, "—" for unreported values) so
 * this component never invents a number.
 */

export type UsageMetric = {
  id: string;
  label: string;
  /** Already formatted. Renders as an em dash when null. */
  value: string | null;
  /** Tooltip explaining why the value is missing or how it was derived. */
  detail?: string;
};

export type UsageMetricsProps = {
  metrics: UsageMetric[];
  /** Balance item, rendered as a refresh button instead of plain text. */
  balance?: { label: string; value: string | null; detail?: string; refreshing?: boolean };
  onRefreshBalance?: () => void;
  cost?: { label: string; value: string | null; detail?: string };
  /**
   * Extra inline items rendered after `cost`, inside the same row. Use for
   * metrics that need richer interaction (e.g. a tooltip wrapper) than the
   * plain label/value shape can express.
   */
  children?: React.ReactNode;
  className?: string;
};

const Item: React.FC<{ label: string; value: string | null; detail?: string }> = ({
  label,
  value,
  detail,
}) => (
  <span className="ui-usage-item" title={detail || undefined}>
    <span className="ui-usage-label">{label}</span> <b>{value ?? "—"}</b>
  </span>
);

export const UsageMetrics: React.FC<UsageMetricsProps> = ({
  metrics,
  balance,
  onRefreshBalance,
  cost,
  children,
  className,
}) => (
  <div className={["ui-usage", className].filter(Boolean).join(" ")} aria-label="当前会话用量">
    {metrics.map((metric) => (
      <Item key={metric.id} label={metric.label} value={metric.value} detail={metric.detail} />
    ))}

    {balance && (
      <Button
        type="button"
        variant="bare"
        className="ui-usage-item ui-usage-action"
        aria-label="刷新账户余额"
        aria-busy={balance.refreshing || undefined}
        title={balance.detail || undefined}
        disabled={balance.refreshing}
        onClick={onRefreshBalance}
      >
        <span className="ui-usage-label">{balance.label}</span> <b>{balance.value ?? "—"}</b>
        {balance.refreshing && <RefreshCw size={10} className="ui-usage-spin" aria-hidden="true" />}
      </Button>
    )}

    {cost && <Item label={cost.label} value={cost.value} detail={cost.detail} />}

    {children}
  </div>
);
