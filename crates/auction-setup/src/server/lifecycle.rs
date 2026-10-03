//! 首次设置的受控连接与有界退出；终止信号不能触发后续数据库初始化。

use auction_process::{DEFAULT_DRAIN_TIMEOUT, DrainOutcome, TerminationSignals, wait_for_drain};
use axum::Router;
use tokio::{net::TcpListener, sync::oneshot};

/// 区分向导完成与操作者终止，不能只凭最终配置存在就继续启动。
pub(super) enum ShutdownReason {
    Completed,
    Interrupted,
}

/// 复用 HTTP 的 5 秒请求头限制与连接任务所有权；停止接收后最多排空 10 秒。
pub(super) async fn serve(
    listener: TcpListener,
    app: Router,
    mut signals: TerminationSignals,
    completed: oneshot::Receiver<()>,
) -> Result<ShutdownReason, &'static str> {
    let (stop_sender, stop_receiver) = oneshot::channel();
    let server = auction_http::serve_http(listener, app, async move {
        let _ = stop_receiver.await;
    });
    tokio::pin!(server);

    let reason = tokio::select! {
        biased;
        signal = signals.recv() => {
            signal.map_err(|_| "cannot receive setup termination signal")?;
            ShutdownReason::Interrupted
        }
        result = &mut server => {
            result.map_err(|_| "setup HTTP server failed")?;
            return Err("setup HTTP server stopped unexpectedly");
        }
        result = completed => {
            result.map_err(|_| "setup completion channel closed")?;
            ShutdownReason::Completed
        }
    };
    let _ = stop_sender.send(());
    let reason = match wait_for_drain(&mut signals, DEFAULT_DRAIN_TIMEOUT, &mut server)
        .await
        .map_err(|_| "setup HTTP drain failed")?
    {
        DrainOutcome::Completed(result) => {
            result.map_err(|_| "setup HTTP server failed")?;
            reason
        }
        DrainOutcome::TimedOut => {
            // server future 拥有连接 JoinSet，离开此作用域会 abort，不能留下 detached 连接。
            // 调用者仍须非阻塞核对持久化终态，超时本身不代表配置写入成功。
            eprintln!("设置服务排空超时，取消剩余连接并核对配置完成状态。");
            reason
        }
        DrainOutcome::Interrupted(_) => ShutdownReason::Interrupted,
    };
    // 共用 drain 优先返回已完成结果；退出前再非阻塞检查一次，不能漏掉同时就绪的终止信号。
    tokio::select! {
        biased;
        signal = signals.recv() => {
            signal.map_err(|_| "cannot receive setup termination signal")?;
            Ok(ShutdownReason::Interrupted)
        }
        () = std::future::ready(()) => Ok(reason),
    }
}
