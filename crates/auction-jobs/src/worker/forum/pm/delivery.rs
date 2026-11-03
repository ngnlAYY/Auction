//! 先准备后持久 fence；sending 后任何错误只留原 sender 只读对账。
use super::*;
use auction_application::{LotWatchError, LotWatchPmClaimRequest, valid_forum_pm_id};

pub(super) async fn process(
    runtime: &LotWatchPmRuntime,
    request: LotWatchPmClaimRequest,
    drain: &watch::Receiver<bool>,
    deadline: tokio::time::Instant,
    activity: &auction_application::ExternalWriteActivity,
) -> Result<(), LotWatchError> {
    check_deadline(deadline)?;
    let Some(mut claim) = runtime.store.claim(request).await? else {
        return Ok(());
    };
    check_deadline(deadline)?;
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
        let receipt =
            runtime.sender.reconcile(claim.world, &claim.message, sender).await.ok().flatten();
        check_deadline(deadline)?;
        return runtime.store.finish(claim, receipt).await;
    }
    if *drain.borrow() {
        if activity.enabled() {
            return Ok(());
        }
        return runtime.store.release_unsent(claim).await;
    }
    let prepared = runtime.sender.prepare(claim.world, &claim.message).await;
    check_deadline(deadline)?;
    let prepared = match prepared {
        Ok(prepared) => prepared,
        // prepare 契约保证只有 GET，只有这阶段才允许安全延期。
        Err(_) => return runtime.store.release_unsent(claim).await,
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
    check_deadline(deadline)?;
    claim.sender = Some(sender);
    // start 提交响应未知直接返回；绝不执行 POST，也绝不释放回 pending。
    let receipt =
        if !activity.enabled() && *drain.borrow() { None } else { prepared.send().await.ok() };
    check_deadline(deadline)?;
    runtime.store.finish(claim, receipt).await?;
    activity.complete();
    Ok(())
}

/// timeout 可能先 poll 出迟到回执；每次阶段交接都复核原截止，过期后只保留持久 claim。
fn check_deadline(deadline: tokio::time::Instant) -> Result<(), LotWatchError> {
    if tokio::time::Instant::now() >= deadline { Err(LotWatchError::Unavailable) } else { Ok(()) }
}
