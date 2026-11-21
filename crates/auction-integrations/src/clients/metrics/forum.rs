//! 论坛请求、私信、写入结果与主题扫描的指标守卫。

use std::{sync::Arc, time::Instant};

use auction_application::external_metrics::{
    ExternalMetricsEvent, ExternalMetricsObserver, ForumOperation, ForumPmCategory, ForumPmOutcome,
    ForumPmPhase, ForumRequestCategory, ForumSignal, ForumTopicScanMeasure, ForumTopicScanMode,
    ForumTopicScanOutcome, ForumWorld, WaitOutcome,
};
use auction_application::forum::ForumWriteProgress;

use super::super::ehentai::EhentaiError;
use super::observation::observe_safely;

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
            observe_safely(
                &self.observer,
                ExternalMetricsEvent::ForumRequestFinished {
                    world: self.world,
                    operation: self.operation,
                    category: ForumRequestCategory::Canceled,
                },
            );
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
            observe_safely(
                &self.observer,
                ExternalMetricsEvent::ForumGateWait {
                    world: self.world,
                    operation: self.operation,
                    outcome: WaitOutcome::Canceled,
                    waited: self.started.elapsed(),
                },
            );
        }
    }
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
    phase: ForumPmPhase,
    drop_unknown: bool,
    completed: bool,
}

impl ForumPmOperationGuard {
    pub(crate) fn start(
        observer: Arc<dyn ExternalMetricsObserver>,
        world: ForumWorld,
        phase: ForumPmPhase,
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

    pub(crate) fn finish(mut self, outcome: ForumPmOutcome, category: ForumPmCategory) {
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
            (ForumPmOutcome::Unknown, ForumPmCategory::OutcomeUnknown)
        } else {
            (ForumPmOutcome::Canceled, ForumPmCategory::None)
        };
        observe_safely(
            &self.observer,
            ExternalMetricsEvent::ForumPmOperation {
                world: self.world,
                phase: self.phase,
                outcome,
                category,
            },
        );
        // POST 已开始后取消同样属于结果未知；未越过 POST 的取消只记 PM canceled。
        if self.drop_unknown && self.phase == ForumPmPhase::Send {
            observe_safely(
                &self.observer,
                ExternalMetricsEvent::ForumWriteUnknown {
                    world: self.world,
                    operation: ForumOperation::PmSend,
                },
            );
        }
    }
}

pub(crate) struct ForumWriteUnknownGuard {
    observer: Arc<dyn ExternalMetricsObserver>,
    world: ForumWorld,
    operation: ForumOperation,
    progress: ForumWriteProgress,
    completed: bool,
}

impl ForumWriteUnknownGuard {
    pub(crate) fn start(
        observer: Arc<dyn ExternalMetricsObserver>,
        world: ForumWorld,
        operation: ForumOperation,
        progress: ForumWriteProgress,
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
        observe_safely(
            &self.observer,
            ExternalMetricsEvent::ForumWriteUnknown {
                world: self.world,
                operation: self.operation,
            },
        );
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
        observe_safely(
            &self.observer,
            ExternalMetricsEvent::ForumTopicScanPages {
                world: self.world,
                mode: self.mode,
                measure,
                outcome,
                pages: self.pages,
            },
        );
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
