//! 提交后提示转为单个合并唤醒；通知不携带任务、不代替持久领取，也不抢占在途发送。

use std::{sync::Arc, time::Duration};

use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_application::delivery::DeliveryWorkStore;
use tokio::sync::{Notify, watch};

use crate::worker::diagnostics;

/// 断线后首次重连前的等待；连续失败按倍数退避。
const RECONNECT_INITIAL: Duration = Duration::from_secs(5);
/// 重连退避上限；订阅只加速唤醒，周期轮询始终兜底。
const RECONNECT_MAXIMUM: Duration = Duration::from_secs(60);
/// 订阅连续保持这么久才视为稳定，再断开时从最短间隔重连；更短的抖动继续退避。
const STABLE_SUBSCRIPTION: Duration = Duration::from_secs(60);

pub(in crate::worker::runtime) async fn run(
    store: Arc<dyn DeliveryWorkStore>,
    wakeup: Arc<Notify>,
    mut drain: watch::Receiver<bool>,
) {
    let mut reconnect = Reconnect::new();
    loop {
        let subscription = tokio::select! {
            biased;
            _ = drain.changed() => return,
            result = store.subscribe_wakeups() => result,
        };
        let mut stable = false;
        match subscription {
            Ok(mut subscription) => {
                if reconnect.connected() {
                    tracing::info!("投递唤醒订阅已恢复");
                }
                let connected_at = tokio::time::Instant::now();
                // 订阅已生效后扫描一次，弥补初次启动/断线窗口中的通知丢失。
                wakeup.notify_one();
                loop {
                    tokio::select! {
                        biased;
                        _ = drain.changed() => return,
                        result = subscription.wait() => {
                            // 订阅等待失败是生产断线路径；记录分类原因后再重连，
                            // 否则诊断窗口只能看到“已断开”而看不到根因。
                            match result {
                                Ok(()) => wakeup.notify_one(),
                                Err(error) => {
                                    diagnostics::warn(
                                        "delivery_wakeup",
                                        "wait_notifications",
                                        error.diagnostic_kind(),
                                        DiagnosticContext::default(),
                                    );
                                    break;
                                }
                            }
                        }
                    }
                }
                stable = connected_at.elapsed() >= STABLE_SUBSCRIPTION;
            }
            Err(ApplicationError::CapabilityUnavailable(_)) => {
                // 不具备订阅能力的实现只保留普通轮询，不通过反复连接伪装具备订阅能力。
                let _ = drain.changed().await;
                return;
            }
            Err(error) => {
                diagnostics::warn(
                    "delivery_wakeup",
                    "subscribe_notifications",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
            }
        }
        let (first_disconnect, delay) = reconnect.disconnected(stable);
        if first_disconnect {
            tracing::warn!("投递唤醒订阅已断开，轮询照常进行");
        }
        tokio::select! {
            biased;
            _ = drain.changed() => return,
            () = tokio::time::sleep(delay) => {}
        }
    }
}

/// 订阅断线后的重连节奏与日志去重：断开只 WARN 一次、恢复只 INFO 一次，期间按指数退避
/// 重连。分类原因由诊断窗口去重记录；状态只影响日志与重连等待，不影响投递领取。
struct Reconnect {
    /// 已记录断开、尚未恢复。
    disconnected: bool,
    /// 下一次重连前的等待。
    delay: Duration,
}

impl Reconnect {
    const fn new() -> Self {
        Self { disconnected: false, delay: RECONNECT_INITIAL }
    }

    /// 订阅已建立；返回是否需要记录一次恢复。
    fn connected(&mut self) -> bool {
        std::mem::replace(&mut self.disconnected, false)
    }

    /// 订阅失败或断开；`stable` 表示刚断开的订阅已稳定保持。返回是否首次记录断开与本次等待。
    fn disconnected(&mut self, stable: bool) -> (bool, Duration) {
        if stable {
            self.delay = RECONNECT_INITIAL;
        }
        let delay = self.delay;
        self.delay = delay.saturating_mul(2).min(RECONNECT_MAXIMUM);
        (!std::mem::replace(&mut self.disconnected, true), delay)
    }
}
