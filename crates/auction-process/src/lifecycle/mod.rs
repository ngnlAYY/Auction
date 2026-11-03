//! 操作系统终止信号、有界 drain 与 Tokio 任务所有权。

mod drain;
pub use drain::{DrainActivity, DrainActivityObserver, DrainCoordinator};

use std::{fmt, future::Future, io, time::Duration};

use tokio::task::{JoinError, JoinHandle};

/// 与旧版进程合同一致的默认 drain 上限。
pub const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminationSignal {
    /// 交互终止信号（SIGINT/Ctrl-C）。
    Interrupt,
    Terminate,
}

impl TerminationSignal {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Interrupt => "sigint",
            Self::Terminate => "sigterm",
        }
    }
}

impl fmt::Display for TerminationSignal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 进程持有的 SIGINT/SIGTERM 订阅。
/// Unix 平台分别订阅两种信号，确保 systemd 的 SIGTERM 与本地 Ctrl-C 进入同一条生命周期；
/// 其他平台使用 Tokio 的 Ctrl-C 抽象，并归类为 SIGINT。
pub struct TerminationSignals {
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
}

impl TerminationSignals {
    /// 在启动依赖装配前订阅终止信号；订阅失败必须阻断启动，不能退回操作系统默认退出路径。
    pub fn subscribe() -> io::Result<Self> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};

            Ok(Self {
                interrupt: signal(SignalKind::interrupt())?,
                terminate: signal(SignalKind::terminate())?,
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {})
        }
    }

    /// 等待下一次终止信号；可连续调用以实现 drain 期间二次信号强制收口。
    pub async fn recv(&mut self) -> io::Result<TerminationSignal> {
        #[cfg(unix)]
        {
            tokio::select! {
                value = self.interrupt.recv() => value
                    .map(|()| TerminationSignal::Interrupt)
                    .ok_or_else(signal_stream_closed),
                value = self.terminate.recv() => value
                    .map(|()| TerminationSignal::Terminate)
                    .ok_or_else(signal_stream_closed),
            }
        }
        #[cfg(not(unix))]
        {
            tokio::signal::ctrl_c().await?;
            Ok(TerminationSignal::Interrupt)
        }
    }
}

fn signal_stream_closed() -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, "termination signal stream closed")
}

#[derive(Debug)]
pub enum DrainOutcome<T> {
    Completed(T),
    /// drain 达到时间上限，调用方必须强制释放自己拥有的资源。
    TimedOut,
    /// drain 被第二次终止信号打断，调用方必须立即强制收口。
    Interrupted(TerminationSignal),
}

/// 等待 drain 完成、第二次终止信号或时间上限三者之一。
/// 返回超时或二次信号时会丢弃 `drain` future；调用方仍负责关闭 listener、终止任务和回收
/// 其他具体资源，避免把 future cancellation 误当作资源已经释放。
pub async fn wait_for_drain<F, T>(
    signals: &mut TerminationSignals,
    timeout: Duration,
    drain: F,
) -> io::Result<DrainOutcome<T>>
where
    F: Future<Output = T>,
{
    if timeout.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "drain timeout must be greater than zero",
        ));
    }

    tokio::pin!(drain);
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    tokio::select! {
        biased;
        signal = signals.recv() => Ok(DrainOutcome::Interrupted(signal?)),
        () = &mut deadline => Ok(DrainOutcome::TimedOut),
        result = &mut drain => {
            // 单次 poll 或调度延迟可能跨过截止；迟到完成不能把超时报告成正常排空。
            if tokio::time::Instant::now() >= deadline.deadline() {
                Ok(DrainOutcome::TimedOut)
            } else {
                Ok(DrainOutcome::Completed(result))
            }
        },
    }
}

/// Tokio 后台任务的 abort-on-drop 所有权守卫。
/// 普通 `JoinHandle` 在丢弃后会让任务脱离；进程 drain future 被超时或二次信号取消时不能留下
/// detached 任务，因此守卫在仍未 join 的情况下自动 abort。
pub struct TaskGuard<T> {
    handle: Option<JoinHandle<T>>,
}

impl<T> TaskGuard<T> {
    pub const fn new(handle: JoinHandle<T>) -> Self {
        Self { handle: Some(handle) }
    }

    /// 请求取消任务；仍应随后调用 `join` 回收任务结果。
    pub fn abort(&self) {
        if let Some(handle) = &self.handle {
            handle.abort();
        }
    }

    /// 等待任务并清除守卫；重复调用返回 `None`。
    /// 若等待本身被取消，handle 仍留在守卫中，随后 Drop 会 abort 任务。
    pub async fn join(&mut self) -> Option<Result<T, JoinError>> {
        let result = {
            let handle = self.handle.as_mut()?;
            handle.await
        };
        self.handle = None;
        Some(result)
    }
}

impl<T> Drop for TaskGuard<T> {
    fn drop(&mut self) {
        if let Some(handle) = &self.handle {
            handle.abort();
        }
    }
}
