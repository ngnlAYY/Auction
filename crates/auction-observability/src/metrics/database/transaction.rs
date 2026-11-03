//! 整操作与单次尝试的独立计量；原 SQLSTATE 在映射前仅投影为闭集元数据。

use std::{
    future::Future,
    sync::{Arc, Mutex, MutexGuard},
};

use super::{
    DatabaseConflictSite, DatabaseRecorder, DatabaseTransactionOwner, DatabaseTransactionPhase,
    labels::ConflictKind,
};

tokio::task_local! {
    static TRANSACTION: Arc<TransactionObservation>;
}

struct TransactionObservation {
    recorder: DatabaseRecorder,
    owner: DatabaseTransactionOwner,
    state: Mutex<ObservationState>,
}

struct ObservationState {
    phase: DatabaseTransactionPhase,
    attempt_active: bool,
    conflict: Option<(ConflictKind, DatabaseConflictSite)>,
    had_conflict: bool,
    exhausted: bool,
}

impl TransactionObservation {
    fn state(&self) -> MutexGuard<'_, ObservationState> {
        // 观测状态不得因别处 panic 改变业务结果；只恢复该计数锁，不恢复事务或业务状态。
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn finish_attempt(&self, state: &mut ObservationState) -> bool {
        if !state.attempt_active {
            return false;
        }
        state.attempt_active = false;
        let Some(metrics) = &self.recorder.0 else { return false };
        let labels = [self.owner.label(), state.phase.label()];
        metrics.attempts.with_label_values(&labels).inc();
        if let Some((kind, site)) = state.conflict.take() {
            metrics
                .conflicts
                .with_label_values(&[
                    self.owner.label(),
                    kind.label(),
                    state.phase.label(),
                    site.label(),
                ])
                .inc();
            state.had_conflict = true;
            return true;
        }
        false
    }
}

struct OperationGuard {
    observation: Arc<TransactionObservation>,
    completed: bool,
}

impl OperationGuard {
    fn finish(&mut self, outcome: &'static str) {
        self.completed = true;
        let mut state = self.observation.state();
        self.observation.finish_attempt(&mut state);
        let Some(metrics) = &self.observation.recorder.0 else { return };
        let owner = self.observation.owner.label();
        let phase = state.phase.label();
        let outcome = if state.exhausted { "exhausted" } else { outcome };
        metrics.transactions.with_label_values(&[owner, outcome, phase]).inc();
        if state.had_conflict {
            metrics.conflicted.with_label_values(&[owner, phase]).inc();
        }
        if state.exhausted {
            metrics.exhausted.with_label_values(&[owner, phase]).inc();
        }
    }
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.finish("canceled");
        }
    }
}

impl DatabaseRecorder {
    /// 观察一个完整 owner 操作；内部重试仍由原 runner 决定，返回值原样传递。
    pub async fn transaction<T, E>(
        &self,
        owner: DatabaseTransactionOwner,
        future: impl Future<Output = Result<T, E>>,
    ) -> Result<T, E> {
        if self.0.is_none() {
            return future.await;
        }
        let observation = Arc::new(TransactionObservation {
            recorder: self.clone(),
            owner,
            state: Mutex::new(ObservationState {
                phase: owner.phase(),
                attempt_active: false,
                conflict: None,
                had_conflict: false,
                exhausted: false,
            }),
        });
        let mut guard = OperationGuard { observation: Arc::clone(&observation), completed: false };
        let result = TRANSACTION.scope(observation, future).await;
        guard.finish(if result.is_ok() { "success" } else { "error" });
        result
    }
}

/// 每次真正进入 BEGIN 获取阶段时调用；重试只在确实开启下一次尝试时累计。
pub fn database_transaction_attempt() {
    let _ = TRANSACTION.try_with(|observation| {
        let mut state = observation.state();
        if observation.finish_attempt(&mut state)
            && let Some(metrics) = &observation.recorder.0
        {
            metrics
                .retries
                .with_label_values(&[observation.owner.label(), state.phase.label()])
                .inc();
        }
        state.phase = observation.owner.phase();
        state.attempt_active = true;
        state.conflict = None;
    });
}

/// 只由 owner 已确认到达原重试上限的分支调用，不推测错误或提交结果。
pub fn database_transaction_exhausted() {
    let _ = TRANSACTION.try_with(|observation| observation.state().exhausted = true);
}

/// 使用本次事务的权威快照完善归属，不额外查库或从缓存猜测 Final。
pub fn database_transaction_phase(phase: DatabaseTransactionPhase) {
    let _ = TRANSACTION.try_with(|observation| observation.state().phase = phase);
}

/// 只接受两个明确中止代码；重复映射同一个错误不重复累计尝试冲突。
pub fn record_database_conflict(sqlstate: &str, site: DatabaseConflictSite) {
    let kind = match sqlstate {
        "40001" => ConflictKind::SerializationFailure,
        "40P01" => ConflictKind::Deadlock,
        _ => return,
    };
    let _ = TRANSACTION.try_with(|observation| {
        let mut state = observation.state();
        if !state.attempt_active {
            return;
        }
        if state.conflict.is_none() || site == DatabaseConflictSite::RealtimeSequence {
            state.conflict = Some((kind, site));
        }
    });
}
