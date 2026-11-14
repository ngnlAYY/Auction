//! MoogleMail Worker 的 claim/续租、批次处理和提交编排；lease fencing 与
//! cursor/事实批次提交顺序由本模块保持。

use std::sync::Arc;

use auction_application::core::Clock;
use auction_application::core::diagnostics::{
    DiagnosticContext, DiagnosticDisposition, DiagnosticError,
};
use auction_application::mail::MailPollClaim;
use auction_observability::WorkerResult;
use chrono::Duration as ChronoDuration;

use super::cycle::MailPollProcessContext;
use crate::worker::diagnostics as worker_diagnostics;

mod business;
mod diagnostics;
mod execute;
mod fetch;
mod progress;
use self::execute::process_claim_inner;
use progress::MailPollProgressGuard;

/// 在整封邮件处理期间维护数据库租约；续租失败时立即取消本地 future，并由 fence 阻止陈旧提交。
pub(super) async fn process_claim<C>(
    context: MailPollProcessContext<C>,
    claim: MailPollClaim,
    mut lease_deadline: tokio::time::Instant,
) -> WorkerResult
where
    C: Clock + 'static,
{
    if !claim.validate() || claim.mailbox_key != context.mailbox_key {
        tracing::error!("MoogleMail Worker 收到无效租约");
        return WorkerResult::Error;
    }
    // 调用方在领取前冻结单调截止；返回租约只能收紧它，不能补回数据库/网络耗时。
    let observed_at = tokio::time::Instant::now();
    let Some(observed_deadline) = (claim.lease_until - context.clock.now())
        .to_std()
        .ok()
        .and_then(|remaining| observed_at.checked_add(remaining))
    else {
        tracing::error!("MoogleMail Worker 收到已过期或不可表示的租约");
        return WorkerResult::Error;
    };
    lease_deadline = lease_deadline.min(observed_deadline);
    let lease_span = context.config.lease_duration;
    let Ok(lease_duration) = ChronoDuration::from_std(context.config.lease_duration) else {
        return WorkerResult::Error;
    };
    let store = Arc::clone(&context.store);
    let clock = Arc::clone(&context.clock);
    let renew_timeout = context.config.renew_interval;
    let mut renewal_claim = claim.clone();
    let mut progress = MailPollProgressGuard::new(
        context.progress_observer.clone(),
        claim.clone(),
        &context.world_id,
    );
    let work = process_claim_inner(context, claim);
    tokio::pin!(work);

    loop {
        let renewal_sleep = tokio::time::sleep(renew_timeout);
        tokio::pin!(renewal_sleep);
        tokio::select! {
            biased;
            () = tokio::time::sleep_until(lease_deadline) => {
                tracing::error!(
                    mailbox_key = %renewal_claim.mailbox_key,
                    "MoogleMail 租约已过期，取消当前陈旧处理"
                );
                return WorkerResult::Error;
            }
            result = std::future::poll_fn(|context| {
                if tokio::time::Instant::now() >= lease_deadline {
                    return std::task::Poll::Pending;
                }
                work.as_mut().poll(context)
            }) => {
                progress.finish(&result);
                return result;
            },
            () = &mut renewal_sleep => {
                let renewal_started = tokio::time::Instant::now();
                if renewal_started >= lease_deadline {
                    tracing::error!(
                        mailbox_key = %renewal_claim.mailbox_key,
                        "MoogleMail 续租开始前租约已过期"
                    );
                    return WorkerResult::Error;
                }
                let renewal_deadline = lease_deadline.min(renewal_started + renew_timeout);
                let renewed_deadline = renewal_started + lease_span;
                let now = clock.now();
                let Some(lease_until) = now.checked_add_signed(lease_duration) else {
                    tracing::error!(mailbox_key = %renewal_claim.mailbox_key, "MoogleMail 续租截止时间溢出");
                    return WorkerResult::Error;
                };
                // 新期限锚定续租发起时刻；等待和回执都不能越过已知旧租约。
                match tokio::time::timeout_at(
                    renewal_deadline,
                    store.renew(renewal_claim.clone(), now, lease_until),
                )
                .await
                {
                    Ok(Ok(updated)) => {
                        if tokio::time::Instant::now() >= renewal_deadline
                            || tokio::time::Instant::now() >= renewed_deadline
                            || clock.now() >= updated.lease_until
                            || updated.lease_until < lease_until
                        {
                            tracing::error!(
                                mailbox_key = %renewal_claim.mailbox_key,
                                "MoogleMail 续租未在原租约内确认有效新期限"
                            );
                            return WorkerResult::Error;
                        }
                        renewal_claim = updated;
                        lease_deadline = renewed_deadline;
                    }
                    Ok(Err(error)) => {
                        worker_diagnostics::error(
                            "mail_poll",
                            "lease_renew",
                            error.diagnostic_kind(),
                            DiagnosticContext {
                                disposition: Some(DiagnosticDisposition::Unpersisted),
                                ..Default::default()
                            },
                        );
                        return WorkerResult::Error;
                    }
                    Err(error) => {
                        worker_diagnostics::unpersisted_error(
                            "mail_poll",
                            "lease_renew_timeout",
                            worker_diagnostics::timeout_kind(&error),
                            DiagnosticContext::default(),
                        );

                        return WorkerResult::Error;
                    }
                }
            }
        }
    }
}
