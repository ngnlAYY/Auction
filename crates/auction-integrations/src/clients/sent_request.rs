//! 已越过发出边界的外部请求：调用方一直等待时就在调用方任务内推进；调用方中途放弃（外层超时、
//! `select!` 落选、任务取消）时，Drop 把尚未完成的剩余部分交给独立任务走完。剩余部分持有的在途锁
//! 等守卫因此保留到远端回应或该次请求自身的 HTTP 预算结束，不随调用方提前释放，紧接的同账号请求
//! 不会与远端仍在处理的请求重叠。
//!
//! 只能包裹发出边界之后的阶段：排队（请求门、在途锁）必须留在调用方 future 内，被放弃即撤票。
//! 正常等待不产生任务，诊断上下文（Tokio task-local）、panic 传播与时序都与直接等待相同；独立任务
//! 只在调用方放弃时产生，每个已发出的请求至多一个，寿命由剩余部分自身的 HTTP 预算约束，没有内在
//! 上限的调用方须给出分离截止。剩余部分在轮询中 panic 或分离时已无可用运行时（进程正在退出），
//! 都直接丢弃，守卫随之释放。

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use tokio::runtime::Handle;
use tokio::time::Instant;

/// 已发出请求的剩余阶段。被放弃后的输出无人接收、直接丢弃，携带敏感正文的输出须在自身 Drop 中
/// 清零。
pub(crate) struct SentRequest<T: Send + 'static> {
    remainder: Option<Pin<Box<dyn Future<Output = T> + Send>>>,
    /// 分离后的寿命上限；`None` 表示剩余部分自身已有有界预算。调用方等待期间不生效。
    detached_deadline: Option<Instant>,
    /// 正在轮询剩余部分；Drop 时仍为真说明轮询中 panic，剩余部分状态已损坏，不再分离。
    polling: bool,
}

impl<T: Send + 'static> SentRequest<T> {
    /// 剩余部分自身有界，如带整次请求超时的 HTTP 交换加上有上限的收尾。
    pub(crate) fn new(remainder: impl Future<Output = T> + Send + 'static) -> Self {
        Self { remainder: Some(Box::pin(remainder)), detached_deadline: None, polling: false }
    }

    /// 剩余部分没有内在上限：被放弃后最多走到 `deadline`，到点丢弃，守卫随之释放。调用方等待期间
    /// 不受此截止约束，仍由调用方自己的预算裁决。
    pub(crate) fn with_detached_deadline(
        remainder: impl Future<Output = T> + Send + 'static,
        deadline: Instant,
    ) -> Self {
        Self {
            remainder: Some(Box::pin(remainder)),
            detached_deadline: Some(deadline),
            polling: false,
        }
    }
}

impl<T: Send + 'static> Future for SentRequest<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let this = self.get_mut();
        // 完成后再被轮询属于误用：保持挂起，不重复执行剩余部分。
        let Some(remainder) = this.remainder.as_mut() else {
            return Poll::Pending;
        };
        this.polling = true;
        let output = remainder.as_mut().poll(cx);
        this.polling = false;
        let output = ready!(output);
        this.remainder = None;
        Poll::Ready(output)
    }
}

impl<T: Send + 'static> Drop for SentRequest<T> {
    fn drop(&mut self) {
        let Some(remainder) = self.remainder.take() else {
            return;
        };
        // 剩余部分在轮询中 panic：再次轮询只会重复 panic，随 remainder 丢弃，守卫立即释放。
        if self.polling {
            return;
        }
        // 分离截止已过：剩余部分到此为止，同样直接丢弃。
        if self.detached_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return;
        }
        // 没有可用运行时（进程正在退出）：同样直接丢弃。运行时正在关闭时 spawn 会立即丢弃任务。
        let Ok(runtime) = Handle::try_current() else {
            return;
        };
        match self.detached_deadline {
            Some(deadline) => {
                runtime.spawn(tokio::time::timeout_at(deadline, remainder));
            }
            None => {
                runtime.spawn(remainder);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll};
    use std::time::Duration;

    use tokio::runtime::Handle;
    use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};
    use tokio::time::Instant;

    use super::SentRequest;

    tokio::task_local! {
        static CALLER: u32;
    }

    /// 代表请求持有的在途锁：容量 1，被持有时可用许可为 0。
    fn held_lock() -> (Arc<Semaphore>, OwnedSemaphorePermit) {
        let lock = Arc::new(Semaphore::new(1));
        let permit = Arc::clone(&lock).try_acquire_owned().expect("锁空闲");
        (lock, permit)
    }

    /// 等到锁重新可用；5 秒内不可用即失败。取得的许可随即归还。
    async fn wait_released(lock: &Semaphore) {
        let _reacquired = tokio::time::timeout(Duration::from_secs(5), lock.acquire())
            .await
            .expect("锁应在 5 秒内释放")
            .expect("信号量未关闭");
    }

    /// 运行时里存活的任务数；测试用单线程运行时，主 future 不计入。
    fn alive_tasks() -> usize {
        Handle::current().metrics().num_alive_tasks()
    }

    /// 等到存活任务数回到 `expected`；5 秒内不成立即失败。
    async fn wait_alive_tasks(expected: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while alive_tasks() != expected {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("独立任务应在 5 秒内结束");
    }

    /// 每次被轮询都计数并 panic，持有守卫直到被丢弃。
    struct PanicsWhenPolled {
        polls: Arc<AtomicUsize>,
        _permit: OwnedSemaphorePermit,
    }

    impl Future for PanicsWhenPolled {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
            self.polls.fetch_add(1, Ordering::SeqCst);
            panic!("剩余部分在轮询中 panic");
        }
    }

    #[tokio::test]
    async fn awaited_request_runs_in_the_callers_task() {
        // 正常等待不分离：剩余部分在调用方任务内执行，看得到调用方的 task-local 上下文，也不产生任务。
        let before = alive_tasks();
        let request = SentRequest::new(async { CALLER.try_with(|caller| *caller).ok() });
        assert_eq!(CALLER.scope(7, request).await, Some(7));
        assert_eq!(alive_tasks(), before);
    }

    #[tokio::test]
    async fn abandoned_request_keeps_its_guard_until_it_finishes() {
        let before = alive_tasks();
        let (lock, permit) = held_lock();
        let (release, released) = oneshot::channel::<()>();
        let (finished, done) = oneshot::channel::<()>();
        let request = SentRequest::new(async move {
            let _permit = permit;
            assert!(released.await.is_ok(), "测试放行剩余部分");
            assert!(finished.send(()).is_ok(), "测试等待剩余部分结束");
        });

        let abandoned = tokio::time::timeout(Duration::from_millis(20), request).await;
        assert!(abandoned.is_err(), "调用方在剩余部分结束前放弃");
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(lock.available_permits(), 0, "调用方放弃后守卫仍由剩余部分持有");
        assert_eq!(alive_tasks(), before + 1, "放弃只产生一个独立任务");

        release.send(()).expect("剩余部分仍在独立任务中等待");
        done.await.expect("剩余部分在独立任务中走完");
        wait_released(&lock).await;
        wait_alive_tasks(before).await;
    }

    #[tokio::test]
    async fn detached_deadline_bounds_only_an_abandoned_remainder() {
        // 调用方等待期间截止不生效：剩余部分超过截止仍照常完成。
        let deadline = Instant::now() + Duration::from_millis(30);
        let waited = SentRequest::with_detached_deadline(
            async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                5
            },
            deadline,
        );
        assert_eq!(waited.await, 5);

        // 截止前被放弃：剩余部分持锁到截止为止，到点丢弃并释放。截止远晚于放弃时刻，调度延迟不会
        // 让放弃落到截止之后。
        let before = alive_tasks();
        let (lock, permit) = held_lock();
        let deadline = Instant::now() + Duration::from_millis(400);
        let request = SentRequest::with_detached_deadline(
            async move {
                let _permit = permit;
                std::future::pending::<()>().await;
            },
            deadline,
        );
        let abandoned = tokio::time::timeout(Duration::from_millis(50), request).await;
        assert!(abandoned.is_err(), "调用方在截止前放弃");
        assert_eq!(lock.available_permits(), 0, "截止前守卫仍由剩余部分持有");
        wait_released(&lock).await;
        assert!(Instant::now() >= deadline, "守卫不得早于分离截止释放");
        wait_alive_tasks(before).await;
    }

    #[tokio::test]
    async fn remainder_abandoned_after_its_deadline_is_dropped_at_once() {
        let before = alive_tasks();
        let (lock, permit) = held_lock();
        let deadline = Instant::now() + Duration::from_millis(30);
        let request = SentRequest::with_detached_deadline(
            async move {
                let _permit = permit;
                std::future::pending::<()>().await;
            },
            deadline,
        );
        let abandoned = tokio::time::timeout(Duration::from_millis(60), request).await;
        assert!(abandoned.is_err(), "调用方在截止后才放弃");
        // 截止已过不再分离：剩余部分随 Drop 丢弃，守卫同步释放，不产生任务。
        assert_eq!(lock.available_permits(), 1);
        assert_eq!(alive_tasks(), before);
    }

    #[tokio::test]
    async fn remainder_that_panics_is_dropped_instead_of_detached() {
        let (lock, permit) = held_lock();
        let polls = Arc::new(AtomicUsize::new(0));
        let request =
            SentRequest::new(PanicsWhenPolled { polls: Arc::clone(&polls), _permit: permit });

        // panic 照常传到等待方；剩余部分不被交给独立任务重新轮询。
        let caller = tokio::spawn(request).await.expect_err("剩余部分 panic");
        assert!(caller.is_panic());
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(polls.load(Ordering::SeqCst), 1, "panic 后不得再轮询剩余部分");
        assert_eq!(lock.available_permits(), 1, "守卫随剩余部分丢弃");
    }
}
