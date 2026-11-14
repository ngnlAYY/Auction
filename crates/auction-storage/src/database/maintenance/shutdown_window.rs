//! 停机前的只读时段证据：Final 与在拍 lot 截止使用同一数据库快照和时钟，
//! 不领取任务、不加行锁或 advisory lock、不改写拍卖、代理或审计事实。

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::Row;
use thiserror::Error;
use uuid::Uuid;

use crate::database::{DatabaseStore, set_read_only_limits};

/// 分体部署停机前的安全投影；只含场次/lot 标识与时间，不含玩家或代理授权信息。
#[derive(Debug, Serialize)]
pub struct PostgresShutdownWindow {
    pub checked_at: DateTime<Utc>,
    pub protected_until: DateTime<Utc>,
    pub final_auctions: Vec<ShutdownFinalAuction>,
    pub ending_lots: Vec<ShutdownEndingLot>,
}

/// 无论有效截止是否落入窗口都保护的 Final 场次。
#[derive(Debug, Serialize)]
pub struct ShutdownFinalAuction {
    pub auction_id: Uuid,
    pub effective_end_at: Option<DateTime<Utc>>,
}

/// 有效截止不晚于保护窗口末端（含已过截止）的在拍 lot。
#[derive(Debug, Serialize)]
pub struct ShutdownEndingLot {
    pub auction_id: Uuid,
    pub lot_id: Uuid,
    pub effective_end_at: DateTime<Utc>,
}

/// 时段证据不可用时只报告固定阶段、SQLx 变体名与校验后的 SQLSTATE，不携带原始错误。
#[derive(Debug, Error)]
#[error(
    "读取 PostgreSQL 停机时段证据失败（stage={stage}, sqlx_kind={sqlx_kind}, sqlstate={sqlstate}）"
)]
pub struct ShutdownWindowError {
    stage: &'static str,
    sqlx_kind: &'static str,
    sqlstate: String,
}

impl DatabaseStore {
    /// 读取 Final、已过截止及窗口内在拍 lot；窗口范围由 CLI 校验，结果不是停机锁或后续状态保证。
    pub async fn collect_shutdown_window(
        &self,
        protection_window_minutes: u32,
    ) -> Result<PostgresShutdownWindow, ShutdownWindowError> {
        let mut transaction = self
            .pool()
            .begin_read_snapshot()
            .await
            .map_err(|error| failure("begin_read_only_transaction", &error))?;
        set_read_only_limits(&mut transaction)
            .await
            .map_err(|(stage, error)| failure(stage, &error))?;

        let clock = sqlx::query(
            "SELECT statement_timestamp() AS checked_at,
                    statement_timestamp() + ($1::bigint * interval '1 minute') AS protected_until",
        )
        .bind(i64::from(protection_window_minutes))
        .fetch_one(&mut *transaction)
        .await
        .map_err(|error| failure("database_clock", &error))?;
        let checked_at: DateTime<Utc> =
            clock.try_get("checked_at").map_err(|error| failure("database_clock", &error))?;
        let protected_until: DateTime<Utc> =
            clock.try_get("protected_until").map_err(|error| failure("database_clock", &error))?;

        let rows = sqlx::query(
            "SELECT id AS auction_id, effective_end_at
             FROM public.auctions WHERE status = 'final' ORDER BY id",
        )
        .fetch_all(&mut *transaction)
        .await
        .map_err(|error| failure("final_auctions", &error))?;
        let mut final_auctions = Vec::with_capacity(rows.len());
        for row in rows {
            final_auctions.push(ShutdownFinalAuction {
                auction_id: row
                    .try_get("auction_id")
                    .map_err(|error| failure("final_auctions", &error))?,
                effective_end_at: row
                    .try_get("effective_end_at")
                    .map_err(|error| failure("final_auctions", &error))?,
            });
        }

        // 已过截止仍为 live 表示生命周期尚未收口，继续阻挡停机，不能按检查时刻排除。
        // 慢速代理沿用同一 lot 截止条件；slow_due_at 本身不阻挡 Live 的其余时段重启。
        // live 缺截止违反已发布数据库不变量，必须进入解码失败，不能被 NULL 比较静默排除。
        let rows = sqlx::query(
            "SELECT auction_id, id AS lot_id, effective_end_at
             FROM public.lots
             WHERE status = 'live'
               AND (effective_end_at <= $1 OR effective_end_at IS NULL)
             ORDER BY effective_end_at, id",
        )
        .bind(protected_until)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|error| failure("ending_lots", &error))?;
        let mut ending_lots = Vec::with_capacity(rows.len());
        for row in rows {
            ending_lots.push(ShutdownEndingLot {
                auction_id: row
                    .try_get("auction_id")
                    .map_err(|error| failure("ending_lots", &error))?,
                lot_id: row.try_get("lot_id").map_err(|error| failure("ending_lots", &error))?,
                effective_end_at: row
                    .try_get("effective_end_at")
                    .map_err(|error| failure("ending_lots", &error))?,
            });
        }
        transaction
            .commit()
            .await
            .map_err(|error| failure("finish_read_only_transaction", &error))?;
        Ok(PostgresShutdownWindow { checked_at, protected_until, final_auctions, ending_lots })
    }
}

fn failure(stage: &'static str, error: &sqlx::Error) -> ShutdownWindowError {
    ShutdownWindowError {
        stage,
        sqlx_kind: crate::database::sql_error_kind(error),
        sqlstate: crate::database::sql_error_sqlstate(error).into_owned(),
    }
}
