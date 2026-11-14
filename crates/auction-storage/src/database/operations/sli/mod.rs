//! SLI SQL 的单一 owner；只读、有界候选、至多一个空闲池连接。
//! 指标不参与业务判定，不取得业务行锁，不写账本、审计或事件。

use auction_application::sli::{
    SLI_CANDIDATE_LIMIT, SLI_WINDOW_SECONDS, SliFlow, SliFlowWindow, SliSnapshot, SliSnapshotError,
    SliSnapshotReader, SliWorld,
};
use chrono::{DateTime, Utc};
use sqlx::postgres::PgRow;
use sqlx::{Connection, PgConnection, Row};

use crate::database::DatabaseStore;

mod capacity;
mod mail_intake;
mod outbid;
mod payout;
mod pending;
mod settlement_mail;
mod slow_history;

/// 每个 world 一个流程窗口，下标与 [`SliWorld::ALL`] 一致。
type WorldWindows = [SliFlowWindow; SliWorld::ALL.len()];

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
    // LOCAL 不改变既有会话分档；正常和异常回滚都恢复原参数。
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
    let window = Window::ending_at(observed_at)?;
    let final_bids = capacity::read(&mut transaction, window).await?;
    let slow_pending = pending::read(&mut transaction, observed_at).await?;
    let slow_history = slow_history::read(&mut transaction, window).await?;
    let outbid = outbid::read(&mut transaction, window).await?;
    let register_key = mail_intake::register_key(&mut transaction, window).await?;
    let submission_take = mail_intake::submission_take(&mut transaction, window).await?;
    let submission_rejection = mail_intake::submission_rejection(&mut transaction, window).await?;
    let (settlement_first, settlement_last) =
        settlement_mail::read(&mut transaction, window).await?;
    let payout = payout::read(&mut transaction, window).await?;
    let mut windows = [[SliFlowWindow::default(); SliFlow::ALL.len()]; SliWorld::ALL.len()];
    for (flow, by_world) in [
        (SliFlow::SlowProxyExecution, slow_history),
        (SliFlow::RegisterKey, register_key),
        (SliFlow::SubmissionTake, submission_take),
        (SliFlow::SubmissionRejectionNotice, submission_rejection),
        (SliFlow::SettlementFirstPaymentMail, settlement_first),
        (SliFlow::SettlementLastPaymentMail, settlement_last),
        (SliFlow::BuyerPaymentToSellerPayout, payout),
    ] {
        for (world, window) in by_world.into_iter().enumerate() {
            windows[world][flow.index()] = window;
        }
    }
    transaction.rollback().await.map_err(storage)?;
    Ok(SliSnapshot { observed_at, final_bids, slow_pending, windows, outbid })
}

/// 一次快照的观察窗口；所有流程共用同一对边界。
#[derive(Clone, Copy)]
struct Window {
    since: DateTime<Utc>,
    now: DateTime<Utc>,
}

impl Window {
    fn ending_at(now: DateTime<Utc>) -> Result<Self, SliSnapshotError> {
        let since = now
            .checked_sub_signed(chrono::Duration::seconds(SLI_WINDOW_SECONDS))
            .ok_or(SliSnapshotError::InvalidData)?;
        Ok(Self { since, now })
    }

    /// 窗口查询的固定绑定：`$1` 起点、`$2` 观察时刻、`$3` 候选上限。
    async fn one(
        self,
        connection: &mut PgConnection,
        sql: &'static str,
    ) -> Result<PgRow, SliSnapshotError> {
        sqlx::query(sql)
            .bind(self.since)
            .bind(self.now)
            .bind(candidate_limit())
            .fetch_one(connection)
            .await
            .map_err(storage)
    }

    async fn all(
        self,
        connection: &mut PgConnection,
        sql: &'static str,
    ) -> Result<Vec<PgRow>, SliSnapshotError> {
        sqlx::query(sql)
            .bind(self.since)
            .bind(self.now)
            .bind(candidate_limit())
            .fetch_all(connection)
            .await
            .map_err(storage)
    }
}

fn candidate_limit() -> i64 {
    // 常量上限与 +1 哨兵允许证明窗口完整，不把 LIMIT 样本冒充总体。
    (SLI_CANDIDATE_LIMIT + 1) as i64
}

/// 任一候选计数触及哨兵即说明窗口被 LIMIT 截断。
fn ensure_complete(row: &PgRow, columns: &[&str]) -> Result<(), SliSnapshotError> {
    for column in columns {
        let candidates: i64 = row.try_get(*column).map_err(storage)?;
        if candidates > SLI_CANDIDATE_LIMIT as i64 {
            return Err(SliSnapshotError::Limited);
        }
    }
    Ok(())
}

/// `invalid` 列统计无法关联到业务事实的候选；出现即拒绝整个快照。
fn ensure_valid(row: &PgRow) -> Result<(), SliSnapshotError> {
    let invalid: i64 = row.try_get("invalid").map_err(storage)?;
    if invalid > 0 { Err(SliSnapshotError::InvalidData) } else { Ok(()) }
}

fn world_index(row: &PgRow) -> Result<usize, SliSnapshotError> {
    let world: String = row.try_get("world_id").map_err(storage)?;
    SliWorld::ALL
        .iter()
        .position(|candidate| candidate.label() == world)
        .ok_or(SliSnapshotError::InvalidData)
}

fn count(row: &PgRow, column: &str) -> Result<u64, SliSnapshotError> {
    let value: i64 = row.try_get(column).map_err(storage)?;
    u64::try_from(value).map_err(|_: std::num::TryFromIntError| SliSnapshotError::InvalidData)
}

/// 样本数与三个分位的列名。
const PERCENTILE_COLUMNS: [&str; 4] = ["samples", "p50", "p95", "p99"];

/// 读取一个已完成样本窗口；未完成覆盖不由历史窗口提供。
fn flow_window(
    row: &PgRow,
    [samples, p50, p95, p99]: [&str; 4],
    unknown: u64,
) -> Result<SliFlowWindow, SliSnapshotError> {
    Ok(SliFlowWindow {
        samples: count(row, samples)?,
        p50: row.try_get(p50).map_err(storage)?,
        p95: row.try_get(p95).map_err(storage)?,
        p99: row.try_get(p99).map_err(storage)?,
        unknown,
        incomplete: 0,
        oldest_incomplete_seconds: None,
        incomplete_available: false,
    })
}

fn timed_out(_: tokio::time::error::Elapsed) -> SliSnapshotError {
    SliSnapshotError::Timeout
}

fn storage(error: sqlx::Error) -> SliSnapshotError {
    let context = crate::database::sql_error_context(&error, "SLI 只读汇总失败");
    tracing::warn!(kind = "sli_storage_failed", phase = "snapshot", context, "保留旧 SLI 快照");
    if error.as_database_error().and_then(sqlx::error::DatabaseError::code).as_deref()
        == Some("57014")
    {
        SliSnapshotError::Timeout
    } else {
        SliSnapshotError::Storage
    }
}
