//! MoogleMail 请求、请求门与在途锁的指标守卫。

use std::{sync::Arc, time::Instant};

use auction_application::external_metrics::{
    ExternalMetricsEvent, ExternalMetricsObserver, MailLock, MailLockOutcome, MailOperation,
    MailRequestCategory, MailSignal, MailWorld, WaitOutcome,
};

use super::super::mooglemail::{MoogleMailOperation, MoogleMailWorld};
use super::mapping::{mail_operation, mail_world};
use super::observation::{observe_dyn_safely, observe_safely};

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
        observe_safely(
            &self.observer,
            ExternalMetricsEvent::MailRequestFinished {
                world: self.world,
                operation: self.operation,
                category,
            },
        );
    }
}

impl Drop for MailRequestGuard {
    fn drop(&mut self) {
        if !self.completed {
            observe_safely(
                &self.observer,
                ExternalMetricsEvent::MailRequestFinished {
                    world: self.world,
                    operation: self.operation,
                    category: MailRequestCategory::Canceled,
                },
            );
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
        observe_safely(
            &self.observer,
            ExternalMetricsEvent::MailGateWait {
                world: self.world,
                operation: self.operation,
                outcome,
                waited: self.started.elapsed(),
            },
        );
    }
}

impl Drop for MailGateWaitGuard {
    fn drop(&mut self) {
        if !self.completed {
            observe_safely(
                &self.observer,
                ExternalMetricsEvent::MailGateWait {
                    world: self.world,
                    operation: self.operation,
                    outcome: WaitOutcome::Canceled,
                    waited: self.started.elapsed(),
                },
            );
        }
    }
}

pub(crate) struct MailLockWaitGuard {
    observer: Arc<dyn ExternalMetricsObserver>,
    world: MailWorld,
    operation: MailOperation,
    lock: MailLock,
    started: Instant,
    completed: bool,
}

impl MailLockWaitGuard {
    pub(crate) fn start(
        observer: Arc<dyn ExternalMetricsObserver>,
        world: MoogleMailWorld,
        operation: MoogleMailOperation,
        lock: MailLock,
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
        observe_safely(
            &self.observer,
            ExternalMetricsEvent::MailLockWait {
                world: self.world,
                operation: self.operation,
                lock: self.lock,
                outcome: MailLockOutcome::Acquired,
                waited: self.started.elapsed(),
            },
        );
    }
}

impl Drop for MailLockWaitGuard {
    fn drop(&mut self) {
        if !self.completed {
            observe_safely(
                &self.observer,
                ExternalMetricsEvent::MailLockWait {
                    world: self.world,
                    operation: self.operation,
                    lock: self.lock,
                    outcome: MailLockOutcome::Canceled,
                    waited: self.started.elapsed(),
                },
            );
        }
    }
}

pub(crate) fn mail_signal(
    observer: &dyn ExternalMetricsObserver,
    world: MoogleMailWorld,
    operation: MoogleMailOperation,
    signal: MailSignal,
) {
    observe_dyn_safely(
        observer,
        ExternalMetricsEvent::MailRequestSignal {
            world: mail_world(world),
            operation: mail_operation(operation),
            signal,
        },
    );
}
