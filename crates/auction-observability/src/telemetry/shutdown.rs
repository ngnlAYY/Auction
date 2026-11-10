//! 进程退出前的 tracing exporter 收口。
//! 角色装配根必须先完成业务 drain，再调用本模块；第二次终止信号直接结束进程，避免可选
//! exporter 拖延 systemd 停止。

use auction_process::{TerminationSignals, join_error_kind};

use super::TracingRuntime;

/// 在有限时间内刷新 tracing；第二次终止信号立即以非零状态结束进程。
pub async fn shutdown_tracing(
    role: &'static str,
    telemetry: TracingRuntime,
    signals: &mut TerminationSignals,
) {
    let result = tokio::select! {
        biased;
        signal = signals.recv() => {
            // spawn_blocking 的刷新无法靠丢弃 future 中断；业务 owner 已完成回收。
            match signal {
                Ok(signal) => tracing::warn!(role, signal = %signal, "tracing 刷新被终止信号强制结束"),
                Err(error) => tracing::warn!(role, error = %error, "tracing 刷新期间信号接收失败"),
            }
            std::process::exit(1);
        }
        result = tokio::task::spawn_blocking(move || telemetry.shutdown()) => result,
    };
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(role, error = %error, "tracing shutdown failed"),
        Err(error) => {
            let kind = join_error_kind(&error);
            tracing::warn!(role, kind, "tracing shutdown task failed");
        }
    }
}
