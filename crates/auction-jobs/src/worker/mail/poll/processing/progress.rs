//! 轮询观测的取消守卫；任何提前返回、任务取消或 panic 都不能遗留“正在处理”。

use std::sync::Arc;

use auction_application::mail::{MailPollClaim, MailPollProgressObserver, MailPollProgressOutcome};
use auction_observability::WorkerResult;

pub(super) struct MailPollProgressGuard {
    observer: Option<Arc<dyn MailPollProgressObserver>>,
    claim: MailPollClaim,
    finished: bool,
}

impl MailPollProgressGuard {
    pub(super) fn new(
        observer: Option<Arc<dyn MailPollProgressObserver>>,
        claim: MailPollClaim,
        world_id: &str,
    ) -> Self {
        if let Some(observer) = &observer {
            observer.begin(&claim, world_id);
        }
        Self { observer, claim, finished: false }
    }

    pub(super) fn finish(&mut self, result: &WorkerResult) {
        if let Some(observer) = &self.observer {
            observer.finish(
                &self.claim,
                match result {
                    WorkerResult::Success => MailPollProgressOutcome::Succeeded,
                    WorkerResult::Error => MailPollProgressOutcome::Failed,
                    WorkerResult::Canceled => MailPollProgressOutcome::Canceled,
                },
            );
        }
        self.finished = true;
    }
}

impl Drop for MailPollProgressGuard {
    fn drop(&mut self) {
        if !self.finished
            && let Some(observer) = &self.observer
        {
            observer.finish(&self.claim, MailPollProgressOutcome::Canceled);
        }
    }
}
