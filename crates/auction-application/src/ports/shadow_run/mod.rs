//! W0.15 影子运行共享契约：新旧判定并行比较、闭集 owner 与分支分类、结构化差异日志与
//! 低基数记录端口。旧路径始终持有裁决权；本模块只记录样本，不返回可影响业务结果的结论。
//!
//! 使用约束：
//! - 调用点必须以旧路径的结果继续控制流，新路径只用于计算待比较的值；
//! - `owner`、`branch` 与不可比较原因都必须是编译期常量，不得放入 MID、账号、正文、
//!   错误原文或业务主键；
//! - 每个 owner 的分支都必须在 [`ShadowOwner::branches`] 登记，指标据此预置零值；
//!   未登记的分支不会被记录，避免出现无覆盖分支被误读为“差异为零”。

use std::sync::Arc;

/// 影子比对的 owner 闭集；新增 owner 在此登记并声明自己的判定分支。
///
/// label 只来自编译期匹配，不接受请求、配置或外部服务返回的字符串。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadowOwner {
    /// W0.14 审计写入失败的就绪门禁：旧口径为任一失败阻断，分类口径只有
    /// permanent/unknown 阻断；瞬时差异是预期差异，其余分支必须为零。
    AuditReadiness,
}

impl ShadowOwner {
    /// 全部 owner；指标按此集合与各 owner 的分支预置零值。
    pub const ALL: [Self; 1] = [Self::AuditReadiness];

    /// 返回编译期固定的 owner 标签。
    pub const fn label(self) -> &'static str {
        match self {
            Self::AuditReadiness => "audit_readiness",
        }
    }

    /// 返回该 owner 的判定分支闭集。
    ///
    /// 切换条件要求每个分支都至少有一个可比较样本，因此预置零值不能当作通过；
    /// 未在此登记的分支不会被记录，也不会出现在指标中。
    pub const fn branches(self) -> &'static [&'static str] {
        match self {
            Self::AuditReadiness => &["transient", "permanent", "unknown"],
        }
    }
}

/// 一次影子样本的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadowOutcome {
    /// 新旧判定一致。
    Match,
    /// 新旧判定不一致，属于必须解释的差异。
    Mismatch,
    /// 新路径未能给出可比较结果（错误、超时、证据不足或前提变化）。
    ///
    /// 不可比较不计入 evaluations，因此不能作为“差异为零”的证据。
    Uncomparable,
}

impl ShadowOutcome {
    /// 返回结构化日志使用的固定标签。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Match => "match",
            Self::Mismatch => "mismatch",
            Self::Uncomparable => "uncomparable",
        }
    }
}

/// 一次影子样本；只含闭集分类与固定标签，不含业务载荷。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadowSample {
    /// 产生样本的功能 owner。
    pub owner: ShadowOwner,
    /// 该 owner 已登记的判定分支。
    pub branch: &'static str,
    /// 本次比对的结论。
    pub outcome: ShadowOutcome,
    /// 不可比较时的闭集原因；一致或差异样本为 `None`。
    ///
    /// 原因必须是调用点的常量（例如错误类别），不得携带错误原文或远端身份。
    pub reason: Option<&'static str>,
}

/// 影子样本的低基数记录端口；实现不得改变调用点的控制流或业务结果。
pub trait ShadowRecorder: Send + Sync {
    /// 记录一次已校验的样本；实现只累加计数，不执行 I/O。
    fn record(&self, sample: ShadowSample);
}

/// 未装配 exporter 时使用；保持比较与日志路径不变。
pub fn noop_shadow_recorder() -> Arc<dyn ShadowRecorder> {
    Arc::new(NoopShadowRecorder)
}

#[derive(Debug, Default)]
struct NoopShadowRecorder;

impl ShadowRecorder for NoopShadowRecorder {
    fn record(&self, _sample: ShadowSample) {}
}

/// 比较新旧判定并记录一次样本；返回值不参与裁决，调用方继续使用旧路径结果。
pub fn record_comparison<T: PartialEq + ?Sized>(
    recorder: &dyn ShadowRecorder,
    owner: ShadowOwner,
    branch: &'static str,
    legacy: &T,
    candidate: &T,
) {
    let outcome = if legacy == candidate { ShadowOutcome::Match } else { ShadowOutcome::Mismatch };
    record_sample(recorder, owner, branch, outcome);
}

/// 记录一次已判定的可比较样本；`branch` 必须在该 owner 的闭集中。
pub fn record_sample(
    recorder: &dyn ShadowRecorder,
    owner: ShadowOwner,
    branch: &'static str,
    outcome: ShadowOutcome,
) {
    record(recorder, ShadowSample { owner, branch, outcome, reason: None });
}

/// 记录一次不可比较样本；它不增加 evaluations，也不构成通过证据。
pub fn record_uncomparable(
    recorder: &dyn ShadowRecorder,
    owner: ShadowOwner,
    branch: &'static str,
    reason: &'static str,
) {
    record(
        recorder,
        ShadowSample { owner, branch, outcome: ShadowOutcome::Uncomparable, reason: Some(reason) },
    );
}

fn record(recorder: &dyn ShadowRecorder, sample: ShadowSample) {
    if !sample.owner.branches().contains(&sample.branch) {
        // 未登记的分支不进入指标，避免动态 label 扩张基数；这属于调用点缺陷。
        tracing::error!(
            target: "auction::shadow",
            owner = sample.owner.label(),
            branch = sample.branch,
            "影子运行分支未在 owner 闭集中登记，样本已丢弃"
        );
        return;
    }
    match sample.outcome {
        ShadowOutcome::Match => tracing::debug!(
            target: "auction::shadow",
            owner = sample.owner.label(),
            branch = sample.branch,
            outcome = sample.outcome.label(),
            "影子判定与旧路径一致"
        ),
        ShadowOutcome::Mismatch => tracing::warn!(
            target: "auction::shadow",
            owner = sample.owner.label(),
            branch = sample.branch,
            outcome = sample.outcome.label(),
            "影子判定与旧路径不一致"
        ),
        // 不可比较代表该分支没有证据，必须可见；样本量大时由调用点用 W0.11 去重。
        ShadowOutcome::Uncomparable => tracing::warn!(
            target: "auction::shadow",
            owner = sample.owner.label(),
            branch = sample.branch,
            outcome = sample.outcome.label(),
            reason = sample.reason.unwrap_or("unclassified"),
            "影子样本不可比较，不计入差异归零证据"
        ),
    }
    recorder.record(sample);
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::{
        ShadowOutcome, ShadowOwner, ShadowRecorder, ShadowSample, record_comparison, record_sample,
        record_uncomparable,
    };

    #[derive(Default)]
    struct CapturingRecorder {
        samples: Mutex<Vec<ShadowSample>>,
    }

    impl ShadowRecorder for CapturingRecorder {
        fn record(&self, sample: ShadowSample) {
            self.samples.lock().expect("capture").push(sample);
        }
    }

    impl CapturingRecorder {
        fn samples(&self) -> Vec<ShadowSample> {
            self.samples.lock().expect("capture").clone()
        }
    }

    #[test]
    fn owner_metadata_is_closed_and_consistent() {
        for owner in ShadowOwner::ALL {
            assert!(!owner.label().is_empty());
            assert!(!owner.label().contains(|ch: char| !(ch.is_ascii_lowercase() || ch == '_')));
            assert!(!owner.branches().is_empty(), "owner 必须登记至少一个分支");
            let mut branches = owner.branches().to_vec();
            branches.sort_unstable();
            branches.dedup();
            assert_eq!(branches.len(), owner.branches().len(), "分支不能重复");
            for branch in owner.branches() {
                assert!(!branch.is_empty());
                assert!(!branch.contains(|ch: char| !(ch.is_ascii_lowercase() || ch == '_')));
            }
        }
        let labels = ShadowOwner::ALL.iter().map(|owner| owner.label()).collect::<Vec<_>>();
        let mut unique = labels.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), labels.len(), "owner 标签不能重复");
    }

    #[test]
    fn comparison_records_match_and_mismatch() {
        let recorder = CapturingRecorder::default();
        record_comparison(&recorder, ShadowOwner::AuditReadiness, "transient", &true, &true);
        record_comparison(&recorder, ShadowOwner::AuditReadiness, "permanent", &true, &false);
        assert_eq!(
            recorder.samples(),
            vec![
                ShadowSample {
                    owner: ShadowOwner::AuditReadiness,
                    branch: "transient",
                    outcome: ShadowOutcome::Match,
                    reason: None,
                },
                ShadowSample {
                    owner: ShadowOwner::AuditReadiness,
                    branch: "permanent",
                    outcome: ShadowOutcome::Mismatch,
                    reason: None,
                },
            ]
        );
    }

    #[test]
    fn uncomparable_keeps_reason_and_outcome() {
        let recorder = CapturingRecorder::default();
        record_uncomparable(&recorder, ShadowOwner::AuditReadiness, "unknown", "timeout");
        let samples = recorder.samples();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].outcome, ShadowOutcome::Uncomparable);
        assert_eq!(samples[0].reason, Some("timeout"));
    }

    #[test]
    fn undeclared_branch_is_dropped() {
        let recorder = CapturingRecorder::default();
        record_sample(
            &recorder,
            ShadowOwner::AuditReadiness,
            "not_declared",
            ShadowOutcome::Mismatch,
        );
        assert!(recorder.samples().is_empty(), "未登记的分支不能进入指标或样本流");
    }
}
