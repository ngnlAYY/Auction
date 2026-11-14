//! 重复只读失败的加密证据配额：同类失败在固定窗口内只保留前几份样本。
//!
//! 远端维护页、协议漂移或网络抖动期间，每条履约义务、每轮维护都会命中同一种失败；
//! 逐次留证会在几十分钟内占满归档的文件数上限，并按时间顺序淘汰其他更有价值的证据。
//! 配额只决定“这次是否另存一份样本”，不改变错误分类、重试或任何业务判定；状态只在
//! 进程内，丢失后最坏是多留几份证据（不变量 5、9）。已进入写阶段的失败、写入未知与
//! 身份类证据不受配额限制，人工对账需要的每一份证据都会保存。

use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};

use auction_application::mail::MailDiagnosticStage;

use super::{MoogleMailError, MoogleMailErrorCategory, MoogleMailOperation, MoogleMailWorld};

/// 配额窗口；与账号忙最长退避同宽，持续故障每小时最多留存约 18 份同类样本。
const WINDOW: Duration = Duration::from_secs(600);

/// 每个窗口内同类失败保留的样本数；多于一份便于区分固定故障页与内容变化。
const SAMPLES_PER_WINDOW: u32 = 3;

/// 同时追踪的失败类别上限；类别是闭集组合，超限只说明状态异常，清空后重新计数。
const MAXIMUM_TRACKED_CLASSES: usize = 256;

/// 世界、操作、阶段与分类；分类与阶段用稳定标签参与比较，不为计数扩展公共枚举的派生。
type ClassKey = (MoogleMailWorld, MoogleMailOperation, &'static str, &'static str);

struct ClassWindow {
    started_at: Instant,
    retained: u32,
}

/// 单个 `MoogleMailClient` 内按失败类别计数；克隆出的客户端共享同一份状态。
#[derive(Default)]
pub(crate) struct MoogleMailEvidenceBudget {
    classes: Mutex<HashMap<ClassKey, ClassWindow>>,
}

impl MoogleMailEvidenceBudget {
    fn classes(&self) -> MutexGuard<'_, HashMap<ClassKey, ClassWindow>> {
        // 配额只影响诊断容量；锁中毒时沿用残留计数，不能把留证升级成进程故障。
        self.classes.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 返回本次失败是否保存证据；不在配额范围内的失败永远返回 `true`。
    pub(crate) fn permits(
        &self,
        stage: MailDiagnosticStage,
        error: &MoogleMailError,
        now: Instant,
    ) -> bool {
        if !repeatable_read_failure(error) {
            return true;
        }
        let mut classes = self.classes();
        if classes.len() >= MAXIMUM_TRACKED_CLASSES {
            classes.clear();
        }
        let window = classes
            .entry((error.world, error.operation, stage_label(stage), error.category.as_str()))
            .or_insert(ClassWindow { started_at: now, retained: 0 });
        if now.saturating_duration_since(window.started_at) >= WINDOW {
            *window = ClassWindow { started_at: now, retained: 0 };
        }
        if window.retained >= SAMPLES_PER_WINDOW {
            return false;
        }
        window.retained += 1;
        true
    }
}

fn stage_label(stage: MailDiagnosticStage) -> &'static str {
    match stage {
        MailDiagnosticStage::Transport => "transport",
        MailDiagnosticStage::Parse => "parse",
        MailDiagnosticStage::Business => "business",
    }
}

/// 只有尚未外写、会在故障期间反复出现的读取失败进入配额。
fn repeatable_read_failure(error: &MoogleMailError) -> bool {
    !error.write_started()
        && matches!(
            error.category,
            MoogleMailErrorCategory::ProtocolDrift
                | MoogleMailErrorCategory::RemoteTemporary
                | MoogleMailErrorCategory::RateLimited
                | MoogleMailErrorCategory::SessionExpired
                | MoogleMailErrorCategory::ResponseTooLarge
        )
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use auction_application::mail::MailDiagnosticStage;

    use super::{MoogleMailEvidenceBudget, SAMPLES_PER_WINDOW, WINDOW};
    use crate::clients::mooglemail::{
        MoogleMailError, MoogleMailErrorCategory, MoogleMailOperation, MoogleMailWorld,
    };

    fn read_error(category: MoogleMailErrorCategory) -> MoogleMailError {
        MoogleMailError::new(MoogleMailOperation::Sent, MoogleMailWorld::Main, category, false)
    }

    fn retained(budget: &MoogleMailEvidenceBudget, error: &MoogleMailError, now: Instant) -> u32 {
        (0..10).map(|_| u32::from(budget.permits(MailDiagnosticStage::Parse, error, now))).sum()
    }

    #[test]
    fn repeated_drift_keeps_a_few_samples_per_window() {
        let budget = MoogleMailEvidenceBudget::default();
        let drift = read_error(MoogleMailErrorCategory::ProtocolDrift);
        let now = Instant::now();
        assert_eq!(retained(&budget, &drift, now), SAMPLES_PER_WINDOW);
        // 窗口过后同类失败重新获得样本额度。
        assert_eq!(retained(&budget, &drift, now + WINDOW), SAMPLES_PER_WINDOW);
    }

    #[test]
    fn classes_are_counted_separately() {
        let budget = MoogleMailEvidenceBudget::default();
        let now = Instant::now();
        let drift = read_error(MoogleMailErrorCategory::ProtocolDrift);
        assert_eq!(retained(&budget, &drift, now), SAMPLES_PER_WINDOW);
        // 其他世界、其他操作、其他阶段与其他分类各自计数。
        let isekai = MoogleMailError::new(
            MoogleMailOperation::Sent,
            MoogleMailWorld::Isekai,
            MoogleMailErrorCategory::ProtocolDrift,
            false,
        );
        let inbox = MoogleMailError::new(
            MoogleMailOperation::Inbox,
            MoogleMailWorld::Main,
            MoogleMailErrorCategory::ProtocolDrift,
            false,
        );
        let temporary = read_error(MoogleMailErrorCategory::RemoteTemporary);
        for error in [&isekai, &inbox, &temporary] {
            assert!(budget.permits(MailDiagnosticStage::Parse, error, now));
        }
        assert!(budget.permits(MailDiagnosticStage::Transport, &drift, now));
        assert!(!budget.permits(MailDiagnosticStage::Parse, &drift, now + Duration::from_secs(1)));
    }

    #[test]
    fn write_and_identity_evidence_is_never_rationed() {
        let budget = MoogleMailEvidenceBudget::default();
        let now = Instant::now();
        let after_write = MoogleMailError::after_write(
            MoogleMailOperation::Take,
            MoogleMailWorld::Main,
            MoogleMailErrorCategory::ProtocolDrift,
        );
        for error in [
            after_write,
            read_error(MoogleMailErrorCategory::WriteUnknown),
            read_error(MoogleMailErrorCategory::IdentityMismatch),
            read_error(MoogleMailErrorCategory::IdentityUnproven),
        ] {
            assert_eq!(retained(&budget, &error, now), 10, "{:?}", error.category);
        }
    }
}
