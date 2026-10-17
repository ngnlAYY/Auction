//! 先准备后持久 fence；sending 后任何错误只留原 sender 只读对账。
use super::*;
use auction_application::{LotWatchError, LotWatchPmClaimRequest, valid_forum_pm_id};

pub(super) async fn process(
    runtime: &LotWatchPmRuntime,
    request: LotWatchPmClaimRequest,
    drain: &watch::Receiver<bool>,
) -> Result<(), LotWatchError> {
    let Some(mut claim) = runtime.store.claim(request).await? else {
        return Ok(());
    };
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
        let receipt = runtime.sender.reconcile(&claim.message, sender).await.ok().flatten();
        return runtime.store.finish(claim, receipt).await;
    }
    if *drain.borrow() {
        return runtime.store.release_unsent(claim).await;
    }
    let prepared = match runtime.sender.prepare(&claim.message).await {
        Ok(prepared) => prepared,
        // prepare 契约保证只有 GET，只有这阶段才允许安全延期。
        Err(_) => return runtime.store.release_unsent(claim).await,
    };
    if *drain.borrow() {
        return runtime.store.release_unsent(claim).await;
    }
    let sender = prepared.sender_forum_account_id().to_owned();
    if !valid_forum_pm_id(&sender) {
        return runtime.store.release_unsent(claim).await;
    }
    if !runtime.store.start_sending(claim.clone(), sender.clone()).await? {
        return Ok(());
    }
    claim.sender = Some(sender);
    // start 提交响应未知直接返回；绝不执行 POST，也绝不释放回 pending。
    let receipt = if *drain.borrow() { None } else { prepared.send().await.ok() };
    runtime.store.finish(claim, receipt).await
}
