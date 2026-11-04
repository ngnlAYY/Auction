//! 外部客户端指标的同步 guard 与闭集映射；不参与请求控制流。

use std::{sync::Arc, time::Instant};

use auction_application::{
    ExternalMetricsEvent, ExternalMetricsObserver, ForumOperation, ForumPmPhase,
    ForumRequestCategory, ForumSignal, ForumTopicScanMeasure, ForumTopicScanMode,
    ForumTopicScanOutcome, ForumWorld, MailLockOutcome, MailOperation, MailRequestCategory,
    MailSignal, MailWorld, WaitOutcome,
};

use super::ehentai::{EhentaiError, EhentaiErrorCategory, EhentaiOperation};
use super::mooglemail::{
    MoogleMailError, MoogleMailErrorCategory, MoogleMailOperation, MoogleMailWorld,
};

pub(crate) fn mail_world(world: MoogleMailWorld) -> MailWorld {
    match world {
        MoogleMailWorld::Main => MailWorld::Main,
        MoogleMailWorld::Isekai => MailWorld::Isekai,
    }
}

pub(crate) fn mail_operation(operation: MoogleMailOperation) -> MailOperation {
    match operation {
        MoogleMailOperation::Inbox => MailOperation::Inbox,
        MoogleMailOperation::ReadMail => MailOperation::ReadMail,
        MoogleMailOperation::Detail => MailOperation::Detail,
        MoogleMailOperation::Sent => MailOperation::Sent,
        MoogleMailOperation::SentDetail => MailOperation::SentDetail,
        MoogleMailOperation::Balance => MailOperation::Balance,
        MoogleMailOperation::Equipment => MailOperation::Equipment,
        MoogleMailOperation::Identity => MailOperation::Identity,
        MoogleMailOperation::Inventory => MailOperation::Inventory,
        MoogleMailOperation::InventoryOrganize => MailOperation::InventoryOrganize,
        MoogleMailOperation::EquipmentUnstore => MailOperation::EquipmentUnstore,
        MoogleMailOperation::EquipmentUnlock => MailOperation::EquipmentUnlock,
        MoogleMailOperation::Compose => MailOperation::Compose,
        MoogleMailOperation::AttachAdd => MailOperation::AttachAdd,
        MoogleMailOperation::AttachCod => MailOperation::AttachCod,
        MoogleMailOperation::Send => MailOperation::Send,
        MoogleMailOperation::ReturnMessage => MailOperation::ReturnMessage,
        MoogleMailOperation::Take => MailOperation::AttachRemove,
        MoogleMailOperation::Discard => MailOperation::Discard,
    }
}

pub(crate) fn mail_category(error: MoogleMailErrorCategory) -> MailRequestCategory {
    match error {
        MoogleMailErrorCategory::Configuration => MailRequestCategory::Configuration,
        MoogleMailErrorCategory::CredentialUnavailable => {
            MailRequestCategory::CredentialUnavailable
        }
        MoogleMailErrorCategory::SessionExpired => MailRequestCategory::SessionExpired,
        MoogleMailErrorCategory::AccountBusy => MailRequestCategory::AccountBusy,
        MoogleMailErrorCategory::RateLimited => MailRequestCategory::RateLimited,
        MoogleMailErrorCategory::RemoteTemporary => MailRequestCategory::RemoteTemporary,
        MoogleMailErrorCategory::SentScanPageLimit => MailRequestCategory::SentScanPageLimit,
        MoogleMailErrorCategory::SentScanTimeout => MailRequestCategory::SentScanTimeout,
        MoogleMailErrorCategory::PersistenceUnavailable => {
            MailRequestCategory::PersistenceUnavailable
        }
        MoogleMailErrorCategory::ResponseTooLarge => MailRequestCategory::ResponseTooLarge,
        MoogleMailErrorCategory::ProtocolDrift => MailRequestCategory::ProtocolDrift,
        MoogleMailErrorCategory::IdentityMismatch => MailRequestCategory::IdentityMismatch,
        MoogleMailErrorCategory::IdentityUnproven => MailRequestCategory::IdentityUnproven,
        MoogleMailErrorCategory::NotObserved => MailRequestCategory::NotObserved,
        MoogleMailErrorCategory::WriteUnknown => MailRequestCategory::WriteUnknown,
    }
}

pub(crate) fn mail_result(result: &Result<Vec<u8>, MoogleMailError>) -> MailRequestCategory {
    match result {
        Ok(_) => MailRequestCategory::Ok,
        Err(error) => mail_category(error.category),
    }
}

pub(crate) fn forum_category(error: EhentaiErrorCategory) -> ForumRequestCategory {
    match error {
        EhentaiErrorCategory::Configuration => ForumRequestCategory::Configuration,
        EhentaiErrorCategory::CredentialUnavailable => ForumRequestCategory::CredentialUnavailable,
        EhentaiErrorCategory::SessionExpired => ForumRequestCategory::SessionExpired,
        EhentaiErrorCategory::RateLimited => ForumRequestCategory::RateLimited,
        EhentaiErrorCategory::RemoteTemporary => ForumRequestCategory::RemoteTemporary,
        EhentaiErrorCategory::Cloudflare => ForumRequestCategory::Cloudflare,
        EhentaiErrorCategory::TopicLocked => ForumRequestCategory::TopicLocked,
        EhentaiErrorCategory::PermissionDenied => ForumRequestCategory::PermissionDenied,
        EhentaiErrorCategory::ResponseTooLarge => ForumRequestCategory::ResponseTooLarge,
        EhentaiErrorCategory::ProtocolDrift => ForumRequestCategory::ProtocolDrift,
        EhentaiErrorCategory::WriteUnknown => ForumRequestCategory::WriteUnknown,
    }
}

pub(crate) fn forum_result<T>(result: &Result<T, EhentaiError>) -> ForumRequestCategory {
    match result {
        Ok(_) => ForumRequestCategory::Ok,
        Err(error) => forum_category(error.category()),
    }
}

pub(crate) fn forum_world(world: auction_domain::SettlementWorld) -> ForumWorld {
    match world {
        auction_domain::SettlementWorld::Main => ForumWorld::Main,
        auction_domain::SettlementWorld::Isekai => ForumWorld::Isekai,
    }
}

pub(crate) fn forum_operation(operation: EhentaiOperation) -> ForumOperation {
    match operation {
        EhentaiOperation::CheckSession => ForumOperation::CheckSession,
        EhentaiOperation::CreateTopic => ForumOperation::CreateTopic,
        EhentaiOperation::ReconcileTopic => ForumOperation::ReconcileTopic,
        EhentaiOperation::FetchTopic => ForumOperation::FetchTopic,
        EhentaiOperation::ReplyTopic => ForumOperation::ReplyTopic,
        EhentaiOperation::EditPost => ForumOperation::EditPost,
        EhentaiOperation::ReadPost => ForumOperation::ReadPost,
    }
}

pub(crate) fn forum_pm_category(
    error: auction_application::ForumPmError,
) -> auction_application::ForumPmCategory {
    use auction_application::{ForumPmCategory, ForumPmError};
    match error {
        ForumPmError::Unavailable => ForumPmCategory::Unavailable,
        ForumPmError::Authentication => ForumPmCategory::Authentication,
        ForumPmError::RateLimited => ForumPmCategory::RateLimited,
        ForumPmError::InvalidRecipient => ForumPmCategory::InvalidRecipient,
        ForumPmError::Protocol => ForumPmCategory::Protocol,
        ForumPmError::OutcomeUnknown => ForumPmCategory::OutcomeUnknown,
    }
}

pub(crate) fn forum_pm_request_result<T>(
    result: &Result<T, auction_application::ForumPmError>,
) -> ForumRequestCategory {
    use auction_application::ForumPmError;
    match result {
        Ok(_) => ForumRequestCategory::Ok,
        Err(ForumPmError::Unavailable) => ForumRequestCategory::CredentialUnavailable,
        Err(ForumPmError::Authentication) => ForumRequestCategory::SessionExpired,
        Err(ForumPmError::RateLimited) => ForumRequestCategory::RateLimited,
        Err(ForumPmError::InvalidRecipient) => ForumRequestCategory::Configuration,
        Err(ForumPmError::Protocol) => ForumRequestCategory::ProtocolDrift,
        Err(ForumPmError::OutcomeUnknown) => ForumRequestCategory::WriteUnknown,
    }
}

pub(crate) struct MailRequestGuard {
    observer: Arc<dyn ExternalMetricsObserver>,
    world: MailWorld,
    operation: MailOperation,
    completed: bool,
}

impl MailRequestGuard {
    pub(crate) fn start(
        observer: Arc<dyn ExternalMetricsObserver>,
        world: MoogleMailWorld,
        operation: MoogleMailOperation,
    ) -> Self {
        let world = mail_world(world);
        let operation = mail_operation(operation);
        observer.observe(ExternalMetricsEvent::MailRequestStarted { world, operation });
        Self { observer, world, operation, completed: false }
    }

    pub(crate) fn finish(mut self, category: MailRequestCategory) {
        self.completed = true;
        self.observer.observe(ExternalMetricsEvent::MailRequestFinished {
            world: self.world,
            operation: self.operation,
            category,
        });
    }
}

impl Drop for MailRequestGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.observer.observe(ExternalMetricsEvent::MailRequestFinished {
                world: self.world,
                operation: self.operation,
                category: MailRequestCategory::Canceled,
            });
        }
    }
}

pub(crate) struct ForumRequestGuard {
    observer: Arc<dyn ExternalMetricsObserver>,
    world: ForumWorld,
    operation: ForumOperation,
    completed: bool,
}

impl ForumRequestGuard {
    pub(crate) fn start(
        observer: Arc<dyn ExternalMetricsObserver>,
        world: ForumWorld,
        operation: ForumOperation,
    ) -> Self {
        observer.observe(ExternalMetricsEvent::ForumRequestStarted { world, operation });
        Self { observer, world, operation, completed: false }
    }

    pub(crate) fn finish(mut self, category: ForumRequestCategory) {
        self.completed = true;
        self.observer.observe(ExternalMetricsEvent::ForumRequestFinished {
            world: self.world,
            operation: self.operation,
            category,
        });
    }
}

impl Drop for ForumRequestGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.observer.observe(ExternalMetricsEvent::ForumRequestFinished {
                world: self.world,
                operation: self.operation,
                category: ForumRequestCategory::Canceled,
            });
        }
    }
}

pub(crate) struct MailGateWaitGuard {
    observer: Arc<dyn ExternalMetricsObserver>,
    world: MailWorld,
    operation: MailOperation,
    started: Instant,
    completed: bool,
}

impl MailGateWaitGuard {
    pub(crate) fn start(
        observer: Arc<dyn ExternalMetricsObserver>,
        world: MoogleMailWorld,
        operation: MoogleMailOperation,
    ) -> Self {
        Self {
            observer,
            world: mail_world(world),
            operation: mail_operation(operation),
            started: Instant::now(),
            completed: false,
        }
    }

    pub(crate) fn finish(mut self, outcome: WaitOutcome) {
        self.completed = true;
        self.observer.observe(ExternalMetricsEvent::MailGateWait {
            world: self.world,
            operation: self.operation,
            outcome,
            waited: self.started.elapsed(),
        });
    }
}

impl Drop for MailGateWaitGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.observer.observe(ExternalMetricsEvent::MailGateWait {
                world: self.world,
                operation: self.operation,
                outcome: WaitOutcome::Canceled,
                waited: self.started.elapsed(),
            });
        }
    }
}

pub(crate) struct ForumGateWaitGuard {
    observer: Arc<dyn ExternalMetricsObserver>,
    world: ForumWorld,
    operation: ForumOperation,
    started: Instant,
    completed: bool,
}

impl ForumGateWaitGuard {
    pub(crate) fn start(
        observer: Arc<dyn ExternalMetricsObserver>,
        world: ForumWorld,
        operation: ForumOperation,
    ) -> Self {
        Self { observer, world, operation, started: Instant::now(), completed: false }
    }

    pub(crate) fn finish(mut self, outcome: WaitOutcome) {
        self.completed = true;
        self.observer.observe(ExternalMetricsEvent::ForumGateWait {
            world: self.world,
            operation: self.operation,
            outcome,
            waited: self.started.elapsed(),
        });
    }
}

impl Drop for ForumGateWaitGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.observer.observe(ExternalMetricsEvent::ForumGateWait {
                world: self.world,
                operation: self.operation,
                outcome: WaitOutcome::Canceled,
                waited: self.started.elapsed(),
            });
        }
    }
}

pub(crate) struct MailLockWaitGuard {
    observer: Arc<dyn ExternalMetricsObserver>,
    world: MailWorld,
    operation: MailOperation,
    lock: auction_application::MailLock,
    started: Instant,
    completed: bool,
}

impl MailLockWaitGuard {
    pub(crate) fn start(
        observer: Arc<dyn ExternalMetricsObserver>,
        world: MoogleMailWorld,
        operation: MoogleMailOperation,
        lock: auction_application::MailLock,
    ) -> Self {
        Self {
            observer,
            world: mail_world(world),
            operation: mail_operation(operation),
            lock,
            started: Instant::now(),
            completed: false,
        }
    }

    pub(crate) fn acquired(mut self) {
        self.completed = true;
        self.observer.observe(ExternalMetricsEvent::MailLockWait {
            world: self.world,
            operation: self.operation,
            lock: self.lock,
            outcome: MailLockOutcome::Acquired,
            waited: self.started.elapsed(),
        });
    }
}

impl Drop for MailLockWaitGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.observer.observe(ExternalMetricsEvent::MailLockWait {
                world: self.world,
                operation: self.operation,
                lock: self.lock,
                outcome: MailLockOutcome::Canceled,
                waited: self.started.elapsed(),
            });
        }
    }
}

pub(crate) fn mail_signal(
    observer: &dyn ExternalMetricsObserver,
    world: MoogleMailWorld,
    operation: MoogleMailOperation,
    signal: MailSignal,
) {
    observer.observe(ExternalMetricsEvent::MailRequestSignal {
        world: mail_world(world),
        operation: mail_operation(operation),
        signal,
    });
}

pub(crate) fn forum_signal(
    observer: &dyn ExternalMetricsObserver,
    world: ForumWorld,
    operation: ForumOperation,
    signal: ForumSignal,
) {
    observer.observe(ExternalMetricsEvent::ForumRequestSignal { world, operation, signal });
}

pub(crate) struct ForumPmOperationGuard {
    observer: Arc<dyn ExternalMetricsObserver>,
    world: ForumWorld,
    phase: auction_application::ForumPmPhase,
    drop_unknown: bool,
    completed: bool,
}

impl ForumPmOperationGuard {
    pub(crate) fn start(
        observer: Arc<dyn ExternalMetricsObserver>,
        world: ForumWorld,
        phase: auction_application::ForumPmPhase,
        drop_unknown: bool,
    ) -> Self {
        Self { observer, world, phase, drop_unknown, completed: false }
    }

    /// 在第一次可能改变远端状态的 POST 发出前调用；取消守卫据此区分未发送与未知结果。
    pub(crate) fn mark_write_started(&mut self) {
        self.drop_unknown = true;
    }

    pub(crate) const fn write_started(&self) -> bool {
        self.drop_unknown
    }

    pub(crate) fn finish(
        mut self,
        outcome: auction_application::ForumPmOutcome,
        category: auction_application::ForumPmCategory,
    ) {
        self.completed = true;
        self.observer.observe(ExternalMetricsEvent::ForumPmOperation {
            world: self.world,
            phase: self.phase,
            outcome,
            category,
        });
    }
}

impl Drop for ForumPmOperationGuard {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        let (outcome, category) = if self.drop_unknown {
            (
                auction_application::ForumPmOutcome::Unknown,
                auction_application::ForumPmCategory::OutcomeUnknown,
            )
        } else {
            (
                auction_application::ForumPmOutcome::Canceled,
                auction_application::ForumPmCategory::None,
            )
        };
        self.observer.observe(ExternalMetricsEvent::ForumPmOperation {
            world: self.world,
            phase: self.phase,
            outcome,
            category,
        });
        // POST 已开始后取消同样属于结果未知；未越过 POST 的取消只记 PM canceled。
        if self.drop_unknown && self.phase == ForumPmPhase::Send {
            self.observer.observe(ExternalMetricsEvent::ForumWriteUnknown {
                world: self.world,
                operation: ForumOperation::PmSend,
            });
        }
    }
}

pub(crate) struct ForumWriteUnknownGuard {
    observer: Arc<dyn ExternalMetricsObserver>,
    world: ForumWorld,
    operation: ForumOperation,
    progress: auction_application::ForumWriteProgress,
    completed: bool,
}

impl ForumWriteUnknownGuard {
    pub(crate) fn start(
        observer: Arc<dyn ExternalMetricsObserver>,
        world: ForumWorld,
        operation: ForumOperation,
        progress: auction_application::ForumWriteProgress,
    ) -> Self {
        Self { observer, world, operation, progress, completed: false }
    }

    pub(crate) fn finish<T>(mut self, result: &Result<T, EhentaiError>) {
        let unknown = matches!(result, Err(error) if error.write_started());
        self.completed = true;
        if unknown {
            self.observe();
        }
    }

    fn observe(&self) {
        self.observer.observe(ExternalMetricsEvent::ForumWriteUnknown {
            world: self.world,
            operation: self.operation,
        });
    }
}

impl Drop for ForumWriteUnknownGuard {
    fn drop(&mut self) {
        if !self.completed && self.progress.has_unconfirmed_write() {
            self.observe();
        }
    }
}

/// 主题扫描的页数观测；增量读取与完整读取分开计数，取消时记录 canceled，不把缺样当零页。
pub(crate) struct ForumTopicScanGuard {
    observer: Arc<dyn ExternalMetricsObserver>,
    world: ForumWorld,
    mode: ForumTopicScanMode,
    pages: usize,
    completed: bool,
}

impl ForumTopicScanGuard {
    pub(crate) fn start(observer: Arc<dyn ExternalMetricsObserver>, world: ForumWorld) -> Self {
        Self { observer, world, mode: ForumTopicScanMode::Full, pages: 0, completed: false }
    }

    pub(crate) fn set_mode(&mut self, mode: ForumTopicScanMode) {
        self.mode = mode;
    }

    pub(crate) fn set_pages(&mut self, pages: usize) {
        self.pages = pages;
    }

    pub(crate) fn finish_complete(mut self) {
        self.completed = true;
        self.observe(ForumTopicScanMeasure::Processed, ForumTopicScanOutcome::Complete);
    }

    pub(crate) fn finish_error(mut self) {
        self.completed = true;
        self.observe(ForumTopicScanMeasure::Processed, ForumTopicScanOutcome::Error);
    }

    fn observe(&self, measure: ForumTopicScanMeasure, outcome: ForumTopicScanOutcome) {
        self.observer.observe(ExternalMetricsEvent::ForumTopicScanPages {
            world: self.world,
            mode: self.mode,
            measure,
            outcome,
            pages: self.pages,
        });
    }
}

impl Drop for ForumTopicScanGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.observe(ForumTopicScanMeasure::Processed, ForumTopicScanOutcome::Canceled);
        }
    }
}

pub(crate) fn topic_scan_discovered(
    observer: &dyn ExternalMetricsObserver,
    world: ForumWorld,
    outcome: ForumTopicScanOutcome,
    pages: usize,
) {
    observer.observe(ExternalMetricsEvent::ForumTopicScanPages {
        world,
        mode: ForumTopicScanMode::Full,
        measure: ForumTopicScanMeasure::Discovered,
        outcome,
        pages,
    });
}
