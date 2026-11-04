//! SLI SQL 的单一 owner；只读、有界候选、至多一个空闲池连接。
//! 指标不参与业务判定，不取得业务行锁，不写账本、审计或事件。

use auction_application::{
    SLI_CANDIDATE_LIMIT, SliFlow, SliFlowWindow, SliSnapshot, SliSnapshotError, SliSnapshotReader,
};
use chrono::{DateTime, Utc};
use sqlx::{Connection, PgConnection};

use super::DatabaseStore;

mod capacity;
mod mail_intake;
mod outbid;
mod payout;
mod pending;
mod settlement_mail;
mod slow_history;

impl SliSnapshotReader for DatabaseStore {
    fn read_sli_snapshot(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<SliSnapshot, SliSnapshotError>> + Send + '_>,
    > {
        Box::pin(async move {
            let Some(mut connection) = self.tracked_postgres_pool().try_acquire() else {
                return Err(SliSnapshotError::Busy);
            };
            let result =
                match tokio::time::timeout(SliSnapshot::TIMEOUT, read(&mut connection)).await {
                    Ok(result) => result,
                    Err(error) => Err(timed_out(error)),
                };
            if result.is_err() {
                // 未确认完整回滚的连接不归还业务池；失败只降级观测。
                connection.close_on_drop();
            }
            result
        })
    }
}

async fn read(connection: &mut PgConnection) -> Result<SliSnapshot, SliSnapshotError> {
    let mut transaction = connection
        .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .await
        .map_err(storage)?;
    // LOCAL 不改变 W0.6 的会话分档；正常和异常回滚都恢复原参数。
    sqlx::query("SET LOCAL statement_timeout = '1000ms'")
        .execute(&mut *transaction)
        .await
        .map_err(storage)?;
    sqlx::query("SET LOCAL lock_timeout = '100ms'")
        .execute(&mut *transaction)
        .await
        .map_err(storage)?;
    let observed_at: DateTime<Utc> = sqlx::query_scalar("SELECT transaction_timestamp()")
        .fetch_one(&mut *transaction)
        .await
        .map_err(storage)?;
    let final_bids = capacity::read(&mut transaction, observed_at).await?;
    let slow_pending = pending::read(&mut transaction, observed_at).await?;
    let slow_history = slow_history::read(&mut transaction, observed_at).await?;
    let outbid = outbid::read(&mut transaction, observed_at).await?;
    let register_key = mail_intake::register_key(&mut transaction, observed_at).await?;
    let submission_take = mail_intake::submission_take(&mut transaction, observed_at).await?;
    let submission_rejection =
        mail_intake::submission_rejection(&mut transaction, observed_at).await?;
    let (settlement_first, settlement_last) =
        settlement_mail::read(&mut transaction, observed_at).await?;
    let payout = payout::read(&mut transaction, observed_at).await?;
    let mut windows =
        [[SliFlowWindow::default(); SliFlow::ALL.len()]; auction_application::SliWorld::ALL.len()];
    for (world, window) in slow_history.into_iter().enumerate() {
        windows[world][SliFlow::SlowProxyExecution.index()] = window;
    }
    for (world, window) in register_key.into_iter().enumerate() {
        windows[world][SliFlow::RegisterKey.index()] = window;
    }
    for (world, window) in submission_take.into_iter().enumerate() {
        windows[world][SliFlow::SubmissionTake.index()] = window;
    }
    for (world, window) in submission_rejection.into_iter().enumerate() {
        windows[world][SliFlow::SubmissionRejectionNotice.index()] = window;
    }
    for (world, window) in settlement_first.into_iter().enumerate() {
        windows[world][SliFlow::SettlementFirstPaymentMail.index()] = window;
    }
    for (world, window) in settlement_last.into_iter().enumerate() {
        windows[world][SliFlow::SettlementLastPaymentMail.index()] = window;
    }
    for (world, window) in payout.into_iter().enumerate() {
        windows[world][SliFlow::BuyerPaymentToSellerPayout.index()] = window;
    }
    transaction.rollback().await.map_err(storage)?;
    Ok(SliSnapshot { observed_at, final_bids, slow_pending, windows, outbid })
}

fn candidate_limit() -> i64 {
    // 常量上限与 +1 哨兵允许证明窗口完整，不把 LIMIT 样本冒充总体。
    (SLI_CANDIDATE_LIMIT + 1) as i64
}

fn timed_out(_: tokio::time::error::Elapsed) -> SliSnapshotError {
    SliSnapshotError::Timeout
}

fn storage(error: sqlx::Error) -> SliSnapshotError {
    let context = super::sql_error_context(&error, "SLI 只读汇总失败");
    tracing::warn!(kind = "sli_storage_failed", phase = "snapshot", context, "保留旧 SLI 快照");
    if error.as_database_error().and_then(|error| error.code()).as_deref() == Some("57014") {
        SliSnapshotError::Timeout
    } else {
        SliSnapshotError::Storage
    }
}
