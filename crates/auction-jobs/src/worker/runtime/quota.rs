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
        Some(Self { permits: Arc::new(Semaphore::new(permits as usize)) })
    }

    /// 申请一个轮次许可；许可存活期间占用配额，Drop 即释放。
    ///
    /// 全局加锁顺序：同时需要许可与其他进程内互斥锁（如邮件世界维护锁）的路径必须先取
    /// 许可、后取锁，持锁期间不得等待许可；否则配额耗尽时持锁等许可的一方会与持许可等锁
    /// 的一方互相等待，并堵住所有共享配额的批量循环。
    pub async fn acquire(&self) -> OwnedSemaphorePermit {
        Arc::clone(&self.permits)
            .acquire_owned()
            .await
            .unwrap_or_else(|_| unreachable!("Worker 连接配额信号量不会被关闭"))
    }

    /// 测试探针：非阻塞申请，配额已满时返回 `None`；生产路径一律经 [`Self::acquire`] 排队。
    #[cfg(test)]
    pub(crate) fn try_acquire(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.permits).try_acquire_owned().ok()
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
        let first = quota.try_acquire();
        let second = quota.try_acquire();
        assert!(first.is_some() && second.is_some(), "全部轮次结束后许可必须释放");
        assert!(quota.try_acquire().is_none(), "释放后上限仍是配置值");
    }

    #[tokio::test(start_paused = true)]
    async fn exhausted_quota_queues_until_permit_released() {
        let quota = Arc::new(WorkerConnectionQuota::new(1).expect("配额上限非零"));
        let held = quota.acquire().await;
        let waiter = tokio::spawn({
            let quota = Arc::clone(&quota);
            async move {
                let _permit = quota.acquire().await;
            }
        });
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(!waiter.is_finished(), "配额已满时申请必须排队等待");
        drop(held);
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("释放后排队者必须取得许可")
            .expect("配额任务不得 panic");
        assert!(quota.try_acquire().is_some(), "排队者结束后许可必须归还");
    }
}
