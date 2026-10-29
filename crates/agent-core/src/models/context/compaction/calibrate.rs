//! Token-estimate calibration and compaction hysteresis.
/// P8-C：token 估算漂移的 EMA 校准器（gemini-cli `adaptiveTokenCalculator`
/// 同款参数：学习率 0.2、单步限幅 0.15、绝对界 0.5–2.0）。
///
/// weight 平滑「provider 实收 / 本地估算」的比值：>1 = 估算偏低（实收更多，
/// 触发点应提前），<1 = 估算偏高（实收更少，触发点可回调——旧实现
/// clamp(100,·) 只缩不放，估算偏大时永不修正）。阻尼与限幅保证单次异常
/// 报告不会让触发点跳变；爆窗安全由 provider 超窗硬压兜底（绝对界 2.0
/// 最多把触发点提前一倍）。
#[derive(Debug, Clone)]
pub struct TokenEstimateCalibrator {
    weight: f64,
}

impl Default for TokenEstimateCalibrator {
    fn default() -> Self {
        Self { weight: 1.0 }
    }
}

impl TokenEstimateCalibrator {
    const LEARNING_RATE: f64 = 0.2;
    const MAX_STEP: f64 = 0.15;
    const BOUND_LOW: f64 = 0.5;
    const BOUND_HIGH: f64 = 2.0;

    /// 用一次「实收 vs 估算」观测更新权重。
    pub fn observe(&mut self, reported: u64, local_estimate: u64) {
        let target = (reported as f64 / local_estimate.max(1) as f64)
            .clamp(Self::BOUND_LOW, Self::BOUND_HIGH);
        let old = self.weight;
        // 单步限幅先作用于目标，再做 EMA——两次阻尼叠加，异常报告的影响被
        // 压到 ~3% 量级。
        let stepped = target.clamp(old - Self::MAX_STEP, old + Self::MAX_STEP);
        self.weight = (old * (1.0 - Self::LEARNING_RATE) + stepped * Self::LEARNING_RATE)
            .clamp(Self::BOUND_LOW, Self::BOUND_HIGH);
    }

    /// 校准后的电荷百分比（provider 实收 ≈ weight × 本地估算）。
    pub fn charge_percent(&self) -> usize {
        (self.weight * 100.0).round() as usize
    }
}

/// P8-E：压缩触发的滞回状态——一次压缩之后，触发线要下移一段安全带，
/// 避免「压完立刻又跨线」的抖动。
///
/// 抖动长什么样：压缩把一个请求降到 79%，而触发线在 80%；下一轮只要多出
/// 2% 的工具输出就再次触发，于是每轮都付一次摘要往返 + checkpoint 重放，
/// 而且每次都让 provider 的 prompt cache 整体失效。滞回记录「上次实际触发
/// 的位置」，下一次的触发线取 `min(计算值, 上次触发位置 − 安全带)`。
///
/// 安全带按百分比给（缺省 10%），不按绝对值：小窗口下 10% 是一两百 token，
/// 大窗口下是几万 token，两者都是同一件事——给压缩后的请求留出继续生长的
/// 空间。
///
/// 存储是按会话的内存状态，与 [`TokenEstimateCalibrator`] 同类。
#[derive(Debug, Clone)]
pub struct CompactionHysteresis {
    last_trigger: Option<usize>,
}

impl Default for CompactionHysteresis {
    fn default() -> Self {
        Self { last_trigger: None }
    }
}

impl CompactionHysteresis {
    /// 安全带占触发线的比例（%）。
    pub const DEFAULT_MARGIN_PERCENT: usize = 10;

    /// 本轮实际生效的触发线：`min(计算值, 上次触发位置 − 安全带)`。
    ///
    /// 上次触发位置还不知道（本会话尚未压缩过）时原样返回计算值。
    pub fn effective_trigger(&self, trigger: usize) -> usize {
        let Some(last) = self.last_trigger else {
            return trigger;
        };
        let margin = last
            .saturating_mul(Self::DEFAULT_MARGIN_PERCENT)
            .saturating_div(100)
            .max(1);
        last.saturating_sub(margin).min(trigger).max(1)
    }

    /// 记录一次实际触发（`limit` 是本轮生效的触发线）。
    pub fn record_trigger(&mut self, limit: usize) {
        self.last_trigger = Some(limit.max(1));
    }

    /// 会话切换/清空时复位。
    pub fn reset(&mut self) {
        self.last_trigger = None;
    }

    /// 上次触发位置（诊断用）。
    pub fn last_trigger(&self) -> Option<usize> {
        self.last_trigger
    }
}
