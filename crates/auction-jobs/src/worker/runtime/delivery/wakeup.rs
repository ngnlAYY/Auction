//! 提交后提示转为单个合并唤醒；通知不携带任务、不代替持久领取，也不抢占在途发送。

use std::{sync::Arc, time::Duration};

use auction_application::{ApplicationError, DeliveryWorkStore};
use tokio::sync::{Notify, watch};

pub(in crate::worker::runtime) async fn run(
    store: Arc<dyn DeliveryWorkStore>,
    wakeup: Arc<Notify>,
    mut drain: watch::Receiver<bool>,
) {
    loop {
        let subscription = tokio::select! {
            biased;
            _ = drain.changed() => return,
            result = store.subscribe_wakeups() => result,
        };
        match subscription {
            Ok(mut subscription) => {
                // 订阅已生效后扫描一次，弥补初次启动/断线窗口中的通知丢失。
                wakeup.notify_one();
                loop {
                    tokio::select! {
                        biased;
                        _ = drain.changed() => return,
                        result = subscription.wait() => {
                            if result.is_err() {
                                break;
                            }
                            wakeup.notify_one();
                        }
                    }
                }
            }
            Err(ApplicationError::CapabilityUnavailable(_)) => {
                // 旧适配器保留普通轮询，不通过反复连接伪装具备订阅能力。
                let _ = drain.changed().await;
                return;
            }
            Err(_) => {}
        }
        tracing::warn!("delivery wakeup disconnected; polling remains active");
        tokio::select! {
            biased;
            _ = drain.changed() => return,
            () = tokio::time::sleep(Duration::from_secs(5)) => {}
        }
    }
}
