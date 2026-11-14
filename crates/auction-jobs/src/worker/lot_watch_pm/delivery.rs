//! 先准备后持久 fence；sending 后任何错误只留原 sender 只读对账。
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError, diagnostic_id};
use auction_application::lot_watch_pm::{LotWatchPmClaimRequest, valid_forum_pm_id};
use auction_application::public::LotWatchError;
use tokio::sync::watch;

use super::LotWatchPmRuntime;
use crate::worker::diagnostics;

pub(super) async fn process(
    runtime: &LotWatchPmRuntime,
    request: LotWatchPmClaimRequest,
    drain: &watch::Receiver<bool>,
    deadline: tokio::time::Instant,
    activity: &auction_application::operations::ExternalWriteActivity,
    claimed_world: &std::sync::Mutex<Option<auction_domain::SettlementWorld>>,
) -> Result<(), LotWatchError> {
    check_deadline(deadline)?;
    let Some(mut claim) = runtime.store.claim(request).await? else {
        return Ok(());
    };
    set_claimed_world(claimed_world, claim.world);
    check_claim_deadline(runtime, claim.world, deadline)?;
    if claim.reconcile {
        let sender = claim
            .sender
            .as_deref()
            .filter(|id| valid_forum_pm_id(id))
            .ok_or(LotWatchError::Unavailable)?;
        // 未找到/错误均不授权重寄。sender 不可用也保留原 sender manual_reconcile。
        if *drain.borrow() {
            // 尚未执行只读请求，不消耗人工核对意图；租约到期后仍可领取。
            return Ok(());
        }
        let receipt = runtime
            .sender
            .reconcile(claim.world, &claim.message, sender)
            .await
            .inspect_err(|error| {
                diagnostics::warn(
                    "forum_pm",
                    "read_sent_receipt",
                    error.diagnostic_kind(),
                    DiagnosticContext {
                        world: Some(claim.world),
                        notification_id: diagnostic_id(&claim.message.notification_id),
                        ..Default::default()
                    },
                );
            })
            .ok()
            .flatten();
        check_claim_deadline(runtime, claim.world, deadline)?;
        let recovered = receipt.is_some();
        let result = runtime.store.finish(claim, receipt).await;
        if result.is_ok() && recovered {
            // 只读核对找到原发送回执：这是同一条被超提醒的送达终态。
            runtime.notifications.observe_stage(
                auction_application::notification_metrics::NotificationKind::LotWatchPm,
                auction_application::notification_metrics::NotificationStage::Sent,
            );
        }
        return result;
    }
    if *drain.borrow() {
        if activity.enabled() {
            return Ok(());
        }
        return runtime.store.release_unsent(claim).await;
    }
    let prepared = runtime.sender.prepare(claim.world, &claim.message).await;
    check_claim_deadline(runtime, claim.world, deadline)?;
    let prepared = match prepared {
        Ok(prepared) => prepared,
        // prepare 契约保证只有 GET，只有这阶段才允许安全延期。
        Err(error) => {
            let diagnostic_kind = error.diagnostic_kind();
            diagnostics::warn(
                "forum_pm",
                "prepare_message",
                error.diagnostic_kind(),
                DiagnosticContext::default(),
            );
            return runtime
                .store
                .release_unsent(claim)
                .await
                .inspect(|()| diagnostics::unpersisted(diagnostic_kind));
        }
    };
    if *drain.borrow() {
        if activity.enabled() {
            return Ok(());
        }
        return runtime.store.release_unsent(claim).await;
    }
    let sender = prepared.sender_forum_account_id().to_owned();
    if !valid_forum_pm_id(&sender) {
        return runtime.store.release_unsent(claim).await;
    }
    // 围栏调用开始即受保护；关闭先赢时不写 sending，原 claim 由租约恢复。
    // 登记只防止交接被停机截断，只有 start_sending 确认提交才授权 POST。
    if !activity.enter_critical() {
        return Ok(());
    }
    if !runtime.store.start_sending(claim.clone(), sender.clone()).await? {
        return Ok(());
    }
    check_claim_deadline(runtime, claim.world, deadline)?;
    claim.sender = Some(sender);
    // start 提交响应未知直接返回；绝不执行 POST，也绝不释放回 pending。
    let receipt = if !activity.enabled() && *drain.borrow() {
        None
    } else {
        prepared
            .send()
            .await
            .inspect_err(|error| {
                diagnostics::warn(
                    "forum_pm",
                    "send_message",
                    error.diagnostic_kind(),
                    DiagnosticContext {
                        world: Some(claim.world),
                        notification_id: diagnostic_id(&claim.message.notification_id),
                        ..Default::default()
                    },
                );
            })
            .ok()
    };
    check_claim_deadline(runtime, claim.world, deadline)?;
    let sent = receipt.is_some();
    runtime.store.finish(claim, receipt).await?;
    // None 表示远端结果未知，不重发：只记录失败原因，仍由人工对账收口。
    if sent {
        runtime.notifications.observe_stage(
            auction_application::notification_metrics::NotificationKind::LotWatchPm,
            auction_application::notification_metrics::NotificationStage::Sent,
        );
    } else {
        runtime.notifications.observe_stage(
            auction_application::notification_metrics::NotificationKind::LotWatchPm,
            auction_application::notification_metrics::NotificationStage::Failed,
        );
        runtime.notifications.observe_failure(
            auction_application::notification_metrics::NotificationKind::LotWatchPm,
            auction_application::notification_metrics::NotificationFailure::WriteUnknown,
        );
    }
    activity.complete();
    Ok(())
}

/// timeout 可能先 poll 出迟到回执；每次阶段交接都复核原截止，过期后只保留持久 claim。
fn check_deadline(deadline: tokio::time::Instant) -> Result<(), LotWatchError> {
    if tokio::time::Instant::now() >= deadline { Err(LotWatchError::Unavailable) } else { Ok(()) }
}

fn check_claim_deadline(
    runtime: &LotWatchPmRuntime,
    world: auction_domain::SettlementWorld,
    deadline: tokio::time::Instant,
) -> Result<(), LotWatchError> {
    if tokio::time::Instant::now() >= deadline {
        runtime
            .observe_budget(world, auction_application::external_metrics::ForumBudgetPhase::PmItem);
        Err(LotWatchError::Unavailable)
    } else {
        Ok(())
    }
}

fn set_claimed_world(
    claimed_world: &std::sync::Mutex<Option<auction_domain::SettlementWorld>>,
    world: auction_domain::SettlementWorld,
) {
    match claimed_world.lock() {
        Ok(mut guard) => *guard = Some(world),
        Err(poisoned) => *poisoned.into_inner() = Some(world),
    }
}
