//! Delivery 工作项结果与批次计量的合并；只消费各任务返回的终态，不改变持久状态。

use std::collections::HashSet;

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticDisposition};
use auction_observability::WorkerResult;

use crate::worker::diagnostics;
use crate::worker::runtime::merge_worker_result;

pub(super) fn merge_delivery_task_result(
    delivery_result: &mut WorkerResult,
    skipped_recipients: &mut HashSet<(String, String)>,
    unavailable_send_worlds: &mut HashSet<String>,
    delivery_count: &mut usize,
    result: Result<DeliveryTaskResult, tokio::task::JoinError>,
) {
    match result {
        Ok(result) => {
            if let Some(world) = result.deferred_world {
                unavailable_send_worlds.insert(world);
                // 只读延后不占发送批次；batch_limit=1 也继续处理 Take/Return 或另一 world。
                *delivery_count = delivery_count.saturating_sub(1);
            }
            if let Some(recipient) = result.skipped_recipient {
                skipped_recipients.insert(recipient);
            }
            merge_worker_result(delivery_result, result.worker_result);
        }
        Err(error) => {
            diagnostics::observe_task_panic("delivery", &error);
            diagnostics::error(
                "delivery",
                "item_join",
                diagnostics::join_kind(&error),
                DiagnosticContext {
                    disposition: if error.is_cancelled() {
                        None
                    } else {
                        Some(DiagnosticDisposition::Unpersisted)
                    },
                    ..Default::default()
                },
            );
            *delivery_result = WorkerResult::Canceled;
        }
    }
}

pub(super) struct DeliveryTaskResult {
    pub(super) worker_result: WorkerResult,
    pub(super) skipped_recipient: Option<(String, String)>,
    pub(super) deferred_world: Option<String>,
}

impl DeliveryTaskResult {
    pub(super) fn completed(worker_result: WorkerResult) -> Self {
        Self { worker_result, skipped_recipient: None, deferred_world: None }
    }
}
