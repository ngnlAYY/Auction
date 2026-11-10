//! Worker 批量与维护循环的连接配额。
//!
//! 连接池是 Worker 全部循环共享的：批量与维护循环（投递、邮件收件与维护、论坛、恢复、玩家
//! 通知、运维告警、实时消费）一旦同时占满连接，拍卖关键循环（生命周期、慢速代理、独立结算、
//! 结算履约）就要排队等待，Final 阶段会直接表现为出价与生命周期推进变慢。
//!
//! 这里用一个进程内信号量给批量轮次设上限：每个批量轮次在开始前获取一个许可，轮次结束后
//! 立即释放；关键循环从不获取许可，因此始终保留配额之外的连接。许可只决定“何时开始一轮
//! 数据库工作”，不承载到期、租约或完成状态（不变量 5）：租约与重试仍由持久化事实决定，
//! 拿不到许可只是推迟本轮开始。

use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// 批量与维护循环共享的连接配额。
#[derive(Debug)]
pub struct WorkerConnectionQuota {
    permits: Arc<Semaphore>,
}

impl WorkerConnectionQuota {
    /// 建立配额；上限为 0 时返回 `None`，调用方必须拒绝该配置而不是退化为“无限制”。
    pub fn new(permits: u32) -> Option<Self> {
        if permits == 0 {
            return None;
        }
        Self::from_usize(permits as usize)
    }

    /// 以 `usize` 建立配额；语义与 [`Self::new`] 一致。
    pub fn from_usize(permits: usize) -> Option<Self> {
        if permits == 0 {
            return None;
        }
        Some(Self { permits: Arc::new(Semaphore::new(permits)) })
    }

    /// 申请一个轮次许可；许可存活期间占用配额，Drop 即释放。
    /// 既不需要配额也不需要取消的调用方使用 [`Self::try_acquire`]，避免在停机时阻塞。
    pub async fn acquire(&self) -> OwnedSemaphorePermit {
        Arc::clone(&self.permits)
            .acquire_owned()
            .await
            .unwrap_or_else(|_| unreachable!("Worker 连接配额信号量不会被关闭"))
    }

    /// 非阻塞申请；配额已满或信号量已关闭时返回 `None`。
    pub fn try_acquire(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.permits).try_acquire_owned().ok()
    }

    /// 配额上限；只用于启动日志与测试断言。
    pub fn permits(&self) -> usize {
        self.permits.available_permits()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use super::WorkerConnectionQuota;

    #[test]
    fn zero_permits_is_rejected() {
        assert!(WorkerConnectionQuota::new(0).is_none());
        assert!(WorkerConnectionQuota::from_usize(0).is_none());
        assert!(WorkerConnectionQuota::new(1).is_some());
    }

    #[tokio::test]
    async fn permits_bound_concurrent_bulk_rounds() {
        let quota = Arc::new(WorkerConnectionQuota::new(2).expect("配额上限非零"));
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let quota = Arc::clone(&quota);
            let in_flight = Arc::clone(&in_flight);
            let peak = Arc::clone(&peak);
            handles.push(tokio::spawn(async move {
                let _permit = quota.acquire().await;
                let current = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(current, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(5)).await;
                in_flight.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for handle in handles {
            handle.await.expect("配额任务不得 panic");
        }
        assert_eq!(peak.load(Ordering::SeqCst), 2, "并发批量轮次不得超过配额");
        assert_eq!(quota.permits(), 2, "全部轮次结束后许可必须释放");
    }

    #[tokio::test]
    async fn try_acquire_does_not_wait_when_exhausted() {
        let quota = WorkerConnectionQuota::new(1).expect("配额上限非零");
        let held = quota.acquire().await;
        assert!(quota.try_acquire().is_none(), "配额已满时非阻塞申请必须失败");
        drop(held);
        assert!(quota.try_acquire().is_some(), "释放后许可应重新可用");
    }
}
