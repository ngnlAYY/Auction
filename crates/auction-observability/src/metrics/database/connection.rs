//! 连接成功取得后的 RAII 计量；守卫随连接或事务移动，不执行释放 SQL。

use std::time::{Duration, Instant};

use super::{DatabaseAcquireOutcome, DatabaseRecorder, DatabaseReservedPurpose};
use crate::WorkerLoop;

/// 跟随物理连接租用生命周期的观测守卫；取消、异常与提前返回均自动扣回 gauge。
pub struct DatabaseConnectionGuard {
    recorder: DatabaseRecorder,
    owner: ConnectionOwner,
    started: Instant,
}

enum ConnectionOwner {
    Worker(WorkerLoop),
    Reserved(DatabaseReservedPurpose),
}

impl DatabaseRecorder {
    /// 计入实际等待结果；未归属和长期 owner 不进入 Worker 直方图。
    pub fn acquire(
        &self,
        loop_kind: Option<WorkerLoop>,
        outcome: DatabaseAcquireOutcome,
        duration: Duration,
    ) {
        if let (Some(metrics), Some(loop_kind)) = (&self.0, loop_kind) {
            metrics
                .acquire_duration
                .with_label_values(&[loop_kind.label(), outcome.label()])
                .observe(duration.as_secs_f64());
        }
    }

    /// 取得普通工作连接后建立持有守卫。
    pub fn hold(&self, loop_kind: Option<WorkerLoop>) -> Option<DatabaseConnectionGuard> {
        let metrics = self.0.as_ref()?;
        let loop_kind = loop_kind?;
        metrics.connections.with_label_values(&[loop_kind.label()]).inc();
        Some(DatabaseConnectionGuard {
            recorder: self.clone(),
            owner: ConnectionOwner::Worker(loop_kind),
            started: Instant::now(),
        })
    }

    /// 长期连接只增加专用占用；不计入普通循环的持有时间。
    pub fn reserve(&self, purpose: DatabaseReservedPurpose) -> Option<DatabaseConnectionGuard> {
        let metrics = self.0.as_ref()?;
        metrics.reserved.with_label_values(&[purpose.label()]).inc();
        Some(DatabaseConnectionGuard {
            recorder: self.clone(),
            owner: ConnectionOwner::Reserved(purpose),
            started: Instant::now(),
        })
    }
}

impl Drop for DatabaseConnectionGuard {
    fn drop(&mut self) {
        let Some(metrics) = &self.recorder.0 else { return };
        match self.owner {
            ConnectionOwner::Worker(loop_kind) => {
                metrics.connections.with_label_values(&[loop_kind.label()]).dec();
                metrics
                    .hold_duration
                    .with_label_values(&[loop_kind.label()])
                    .observe(self.started.elapsed().as_secs_f64());
            }
            ConnectionOwner::Reserved(purpose) => {
                metrics.reserved.with_label_values(&[purpose.label()]).dec();
            }
        }
    }
}
