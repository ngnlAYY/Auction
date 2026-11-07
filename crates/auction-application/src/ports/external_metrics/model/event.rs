//! 同步观测事件：调用点在既有时钟与已观察页数上的投影。

use std::time::Duration;

use super::forum::{ForumBudgetPhase, ForumOperation, ForumRequestCategory, ForumSignal};
use super::mail::{
    MailBudget, MailNoticeClass, MailOperation, MailPhase, MailRequestCategory, MailSignal,
};
use super::pm::{ForumPmCategory, ForumPmOutcome, ForumPmPhase};
use super::reply_delay::{ReplyDelayMethod, ReplyDelayPhase, ReplyDelaySampleOutcome};
use super::topic::{ForumTopicScanMeasure, ForumTopicScanMode, ForumTopicScanOutcome};
use super::wait::{MailLock, MailLockOutcome, WaitOutcome};
use super::world::{ForumWorld, MailWorld};

/// 同步观测事件；数值只来自调用点已有的时钟或已观察页数。
#[derive(Debug, Clone, Copy)]
pub enum ExternalMetricsEvent {
    MailRequestStarted {
        world: MailWorld,
        operation: MailOperation,
    },
    MailRequestFinished {
        world: MailWorld,
        operation: MailOperation,
        category: MailRequestCategory,
    },
    MailRequestSignal {
        world: MailWorld,
        operation: MailOperation,
        signal: MailSignal,
    },
    MailGateWait {
        world: MailWorld,
        operation: MailOperation,
        outcome: WaitOutcome,
        waited: Duration,
    },
    MailLockWait {
        world: MailWorld,
        operation: MailOperation,
        lock: MailLock,
        outcome: MailLockOutcome,
        waited: Duration,
    },
    MailBudgetExhausted {
        world: MailWorld,
        phase: MailPhase,
        budget: MailBudget,
    },
    MailRequestInterval {
        interval: Duration,
    },
    ForumRequestStarted {
        world: ForumWorld,
        operation: ForumOperation,
    },
    ForumRequestFinished {
        world: ForumWorld,
        operation: ForumOperation,
        category: ForumRequestCategory,
    },
    ForumRequestSignal {
        world: ForumWorld,
        operation: ForumOperation,
        signal: ForumSignal,
    },
    ForumGateWait {
        world: ForumWorld,
        operation: ForumOperation,
        outcome: WaitOutcome,
        waited: Duration,
    },
    ForumRequestInterval {
        interval: Duration,
    },
    ForumTopicScanPages {
        world: ForumWorld,
        mode: ForumTopicScanMode,
        measure: ForumTopicScanMeasure,
        outcome: ForumTopicScanOutcome,
        pages: usize,
    },
    ForumWriteUnknown {
        world: ForumWorld,
        operation: ForumOperation,
    },
    ForumPmOperation {
        world: ForumWorld,
        phase: ForumPmPhase,
        outcome: ForumPmOutcome,
        category: ForumPmCategory,
    },
    ForumBudgetExhausted {
        world: ForumWorld,
        phase: ForumBudgetPhase,
    },
    ForumReplyDelayEstimate {
        world: ForumWorld,
        phase: ReplyDelayPhase,
        method: ReplyDelayMethod,
        lower: Duration,
        midpoint: Duration,
        upper: Duration,
    },
    ForumReplyDelaySample {
        world: ForumWorld,
        phase: ReplyDelayPhase,
        method: ReplyDelayMethod,
        outcome: ReplyDelaySampleOutcome,
    },
    /// 说明信或账号回信因 24 小时预算被抑制；入站命令与审计仍已记录。
    MailNoticeSuppressed {
        world: MailWorld,
        class: MailNoticeClass,
    },
}
