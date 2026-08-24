//! SQLite Lite 档竞价事务。
//!
//! SQLite 没有 `FOR UPDATE`。整个写路径先取得连接并执行 `BEGIN IMMEDIATE`，
//! 由 SQLite 的单写者锁保护 lot、代理、命令账本和事件写入。

use std::{collections::HashSet, future::Future};

use auction_application::{
    AuctionLifecycleCommand, AuctionLifecycleContext, AuctionLifecycleEffect, AuctionLifecyclePlan,
    AuctionLifecycleResult, AuctionLifecycleTransaction, AuctionPreviewSuccessorPlan,
    CommandLedgerError, CompetitionSnapshot, CompetitionTransaction, CompetitionTransactionError,
    RequestHash, StoredCommand, TransactionFailure, TransactionFuture, TransactionIsolation,
    TransactionPolicy, TransactionRunner,
};
use auction_domain::{
    AcceptedBid, Auction, AuctionId, AuctionRules, AuctionStatus, BidAmount, BidMechanism,
    CompetitionPlan, LotBidState, LotId, ProxyAuthorization, ProxyAuthorizationId, ProxyMode,
    ProxyStatus, ProxyTerminalReason, UserId,
};
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Utc};
use serde_json::{Value, json};
use sqlx::{Row, Sqlite, SqliteConnection, pool::PoolConnection, sqlite::SqliteRow};
use uuid::Uuid;

use super::super::SqliteStore;

/// SQLite 竞价事务持有的唯一连接。
///
/// 连接在 `BEGIN IMMEDIATE` 到提交/回滚之间不能交还连接池，避免同一命令的账本、
/// lot 状态和追加事实被拆到不同事务。
pub struct SqliteCompetitionTransaction {
    connection: Option<PoolConnection<Sqlite>>,
}

impl SqliteCompetitionTransaction {
    async fn begin(connection: PoolConnection<Sqlite>) -> Result<Self, sqlx::Error> {
        let mut transaction = Self { connection: Some(connection) };
        sqlx::query("BEGIN IMMEDIATE")
            .execute(
                transaction
                    .connection_mut()
                    .map_err(|_| sqlx::Error::Protocol("SQLite 事务连接不可用".to_owned()))?,
            )
            .await?;
        Ok(transaction)
    }

    async fn commit(mut self) -> Result<(), sqlx::Error> {
        let Some(mut connection) = self.connection.take() else {
            return Ok(());
        };
        let result = sqlx::query("COMMIT").execute(&mut *connection).await;
        if result.is_err() {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
        }
        result.map(|_| ())
    }

    async fn rollback(mut self) -> Result<(), sqlx::Error> {
        let Some(mut connection) = self.connection.take() else {
            return Ok(());
        };
        sqlx::query("ROLLBACK").execute(&mut *connection).await.map(|_| ())
    }

    fn connection_mut(&mut self) -> Result<&mut SqliteConnection, CompetitionTransactionError> {
        self.connection
            .as_deref_mut()
            .ok_or_else(|| CompetitionTransactionError::Failed("竞价事务已经结束".to_owned()))
    }
}

/// SQLite 使用 `BEGIN IMMEDIATE` 把 Serializable 竞价写入映射为单写者事务。
#[allow(clippy::manual_async_fn)]
impl TransactionRunner for SqliteStore {
    type Transaction = SqliteCompetitionTransaction;

    fn run<T, E, F>(
        &self,
        policy: TransactionPolicy,
        operation: F,
    ) -> impl Future<Output = Result<T, E>> + Send
    where
        T: Send,
        E: TransactionFailure,
        F: for<'a> Fn(&'a mut Self::Transaction) -> TransactionFuture<'a, T, E> + Send,
    {
        async move {
            if let Err(error) = TransactionPolicy::new(policy.isolation, policy.max_attempts) {
                return Err(E::from(error));
            }

            // Lite 档的两个隔离枚举都由同一个 SQLite 写锁入口承载；不在这里伪造
            // PostgreSQL 的隔离级别，而是保留应用层策略并显式执行 IMMEDIATE。
            let _isolation = match policy.isolation {
                TransactionIsolation::Serializable => "serializable",
                TransactionIsolation::ReadCommitted => "read_committed",
            };

            for attempt in 0..policy.max_attempts {
                let connection = match self.pool().acquire().await {
                    Ok(connection) => connection,
                    Err(error) => {
                        let mapped = map_sql_error(error);
                        if matches!(mapped, CompetitionTransactionError::SerializationConflict)
                            && attempt + 1 < policy.max_attempts
                        {
                            continue;
                        }
                        return Err(E::from(mapped));
                    }
                };
                let mut transaction = match Self::Transaction::begin(connection).await {
                    Ok(transaction) => transaction,
                    Err(error) => {
                        let mapped = map_sql_error(error);
                        if matches!(mapped, CompetitionTransactionError::SerializationConflict)
                            && attempt + 1 < policy.max_attempts
                        {
                            continue;
                        }
                        return Err(E::from(mapped));
                    }
                };

                match operation(&mut transaction).await {
                    Ok(value) => match transaction.commit().await {
                        Ok(()) => return Ok(value),
                        Err(error) => {
                            let mapped = map_sql_error(error);
                            if matches!(mapped, CompetitionTransactionError::SerializationConflict)
                                && attempt + 1 < policy.max_attempts
                            {
                                continue;
                            }
                            return Err(E::from(mapped));
                        }
                    },
                    Err(error) => {
                        let retryable = error.is_serialization_conflict();
                        let _ = transaction.rollback().await;
                        if retryable && attempt + 1 < policy.max_attempts {
                            continue;
                        }
                        return Err(error);
                    }
                }
            }

            Err(E::from(CompetitionTransactionError::SerializationConflict))
        }
    }
}

#[allow(clippy::manual_async_fn)]
impl CompetitionTransaction for SqliteCompetitionTransaction {
    fn find_command(
        &mut self,
        command_id: auction_domain::CommandId,
    ) -> impl Future<Output = Result<Option<StoredCommand>, CompetitionTransactionError>> + Send
    {
        async move { self.find_command_inner(command_id).await }
    }

    fn insert_command(
        &mut self,
        command: StoredCommand,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        async move { self.insert_command_inner(command).await }
    }

    fn load_for_update(
        &mut self,
        lot_id: LotId,
    ) -> impl Future<Output = Result<CompetitionSnapshot, CompetitionTransactionError>> + Send {
        async move { self.load_for_update_inner(lot_id).await }
    }

    fn next_proxy_usage_number(
        &mut self,
        lot_id: LotId,
        user_id: UserId,
    ) -> impl Future<Output = Result<u64, CompetitionTransactionError>> + Send {
        async move { self.next_proxy_usage_number_inner(lot_id, user_id).await }
    }

    fn apply_plan(
        &mut self,
        plan: CompetitionPlan,
        request_id: Option<String>,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        async move { self.apply_plan_inner(plan, request_id).await }
    }
}

#[allow(clippy::manual_async_fn)]
impl AuctionLifecycleTransaction for SqliteCompetitionTransaction {
    fn load_lifecycle_context_for_update(
        &mut self,
        auction_id: AuctionId,
    ) -> impl Future<Output = Result<Option<AuctionLifecycleContext>, CompetitionTransactionError>> + Send
    {
        async move { load_lifecycle_context_sqlite(self, auction_id).await }
    }

    fn apply_lifecycle_plan(
        &mut self,
        command: &AuctionLifecycleCommand,
        plan: AuctionLifecyclePlan,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        let command = command.clone();
        async move { apply_lifecycle_plan_sqlite(self, &command, plan).await }
    }

    fn list_final_lot_ids_for_update(
        &mut self,
        auction_id: AuctionId,
    ) -> impl Future<Output = Result<Vec<LotId>, CompetitionTransactionError>> + Send {
        async move {
            let rows = {
                let connection = self.connection_mut()?;
                sqlx::query(
                    "SELECT id FROM lots
                     WHERE auction_id = ? AND status = 'live'
                     ORDER BY lot_number",
                )
                .bind(uuid_blob(auction_id.as_uuid()))
                .fetch_all(&mut *connection)
                .await
                .map_err(map_sql_error)?
            };
            rows.into_iter()
                .map(|row| {
                    uuid_from_blob(row.try_get("id").map_err(map_sql_error)?, "Lite final lot 标识")
                        .map(LotId::from_uuid)
                })
                .collect()
        }
    }

    fn proxy_user_is_eligible_for_update(
        &mut self,
        user_id: UserId,
    ) -> impl Future<Output = Result<bool, CompetitionTransactionError>> + Send {
        async move {
            let status = {
                let connection = self.connection_mut()?;
                sqlx::query_scalar::<_, String>("SELECT status FROM users WHERE id = ?")
                    .bind(uuid_blob(user_id.as_uuid()))
                    .fetch_optional(&mut *connection)
                    .await
                    .map_err(map_sql_error)?
                    .ok_or_else(|| invalid_state("Lite 活动代理用户不存在"))?
            };
            let blacklist_active = {
                let connection = self.connection_mut()?;
                sqlx::query_scalar::<_, i64>(
                    "SELECT CASE WHEN revoked_at IS NULL THEN 1 ELSE 0 END
                     FROM user_blacklist
                     WHERE user_id = ?",
                )
                .bind(uuid_blob(user_id.as_uuid()))
                .fetch_optional(&mut *connection)
                .await
                .map_err(map_sql_error)?
                .unwrap_or(0)
                    != 0
            };
            Ok(status == "active" && !blacklist_active)
        }
    }

    fn append_lifecycle_record(
        &mut self,
        command: &AuctionLifecycleCommand,
        result: &AuctionLifecycleResult,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        async move { append_lifecycle_record_inner(self, command, result).await }
    }
}

async fn load_lifecycle_context_sqlite(
    transaction: &mut SqliteCompetitionTransaction,
    auction_id: AuctionId,
) -> Result<Option<AuctionLifecycleContext>, CompetitionTransactionError> {
    let row = {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "SELECT
                a.id, a.title, a.status, a.state_version, a.created_at, a.updated_at,
                a.scheduled_end_at, a.effective_end_at,
                a.cancelled_at, a.cancelled_reason, a.cancelled_by,
                a.world_id, a.sequence, a.preview_scheduled_at, a.scheduled_open_at,
                a.preview_manual_open,
                json_extract(a.policy_snapshot, '$.scheduled_end_at') AS policy_scheduled_end_at,
                CAST(json_extract(a.policy_snapshot, '$.final_phase_lead_seconds') AS INTEGER)
                    AS policy_final_phase_lead_seconds,
                (SELECT round.local_week FROM scheduled_rounds AS round
                 WHERE round.auction_id = a.id LIMIT 1) AS scheduled_round_local_week
             FROM auctions AS a
             WHERE a.id = ?",
        )
        .bind(uuid_blob(auction_id.as_uuid()))
        .fetch_optional(&mut *connection)
        .await
        .map_err(map_sql_error)?
    };
    let Some(row) = row else {
        return Ok(None);
    };
    let id = uuid_from_blob(row.try_get("id").map_err(map_sql_error)?, "Lite 拍卖标识格式无效")?;
    let auction = Auction {
        id: AuctionId::from_uuid(id),
        title: row.try_get("title").map_err(map_sql_error)?,
        status: parse_auction_status(row.try_get::<String, _>("status").map_err(map_sql_error)?)?,
        state_version: positive_u64(
            row.try_get("state_version").map_err(map_sql_error)?,
            "Lite 拍卖状态版本无效",
        )?,
        created_at: parse_timestamp(
            row.try_get("created_at").map_err(map_sql_error)?,
            "Lite 拍卖创建时间格式无效",
        )?,
        updated_at: parse_timestamp(
            row.try_get("updated_at").map_err(map_sql_error)?,
            "Lite 拍卖更新时间格式无效",
        )?,
        scheduled_end_at: parse_optional_timestamp(
            row.try_get("scheduled_end_at").map_err(map_sql_error)?,
            "Lite 拍卖名义结束时间格式无效",
        )?,
        effective_end_at: parse_optional_timestamp(
            row.try_get("effective_end_at").map_err(map_sql_error)?,
            "Lite 拍卖有效结束时间格式无效",
        )?,
        cancelled_at: parse_optional_timestamp(
            row.try_get("cancelled_at").map_err(map_sql_error)?,
            "Lite 拍卖取消时间格式无效",
        )?,
        cancelled_reason: row.try_get("cancelled_reason").map_err(map_sql_error)?,
        cancelled_by: row.try_get("cancelled_by").map_err(map_sql_error)?,
    };
    let preview_scheduled_at = parse_timestamp(
        row.try_get("preview_scheduled_at").map_err(map_sql_error)?,
        "Lite 预览计划时间格式无效",
    )?;
    let scheduled_open_at = parse_timestamp(
        row.try_get("scheduled_open_at").map_err(map_sql_error)?,
        "Lite 开拍计划时间格式无效",
    )?;
    let policy_scheduled_end_at = parse_timestamp(
        row.try_get("policy_scheduled_end_at").map_err(map_sql_error)?,
        "Lite 策略结束时间格式无效",
    )?;
    let final_phase_lead_seconds: i64 =
        row.try_get("policy_final_phase_lead_seconds").map_err(map_sql_error)?;
    if final_phase_lead_seconds <= 0 {
        return Err(invalid_state("Lite 最终阶段领先时间无效"));
    }
    let scheduled_round_local_week = row
        .try_get::<Option<String>, _>("scheduled_round_local_week")
        .map_err(map_sql_error)?
        .map(|value| {
            NaiveDate::parse_from_str(&value, "%Y-%m-%d")
                .map_err(|_| invalid_state("Lite 周轮次日期格式无效"))
        })
        .transpose()?;
    Ok(Some(AuctionLifecycleContext {
        auction,
        world_id: row.try_get("world_id").map_err(map_sql_error)?,
        sequence: row.try_get("sequence").map_err(map_sql_error)?,
        preview_scheduled_at,
        scheduled_open_at,
        policy_scheduled_end_at,
        policy_final_phase_lead: Duration::seconds(final_phase_lead_seconds),
        preview_manual_open: row.try_get::<i64, _>("preview_manual_open").map_err(map_sql_error)?
            != 0,
        scheduled_round_local_week,
    }))
}

async fn apply_lifecycle_plan_sqlite(
    transaction: &mut SqliteCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    plan: AuctionLifecyclePlan,
) -> Result<(), CompetitionTransactionError> {
    if plan.auction.id != command.auction_id || plan.effect.action() != command.action {
        return Err(invalid_state("Lite 生命周期命令与动作计划不一致"));
    }
    let AuctionLifecyclePlan { auction, effect } = plan;
    match effect {
        AuctionLifecycleEffect::Preview { manual, successor } => {
            apply_preview_plan_sqlite(transaction, command, &auction, manual, &successor).await
        }
        AuctionLifecycleEffect::Open { manual_override } => {
            apply_open_plan_sqlite(transaction, command, &auction, manual_override).await
        }
        AuctionLifecycleEffect::Cancel => apply_cancel_plan_sqlite(transaction, &auction).await,
        AuctionLifecycleEffect::FinalPhase { proxy_plans } => {
            apply_final_plan_sqlite(transaction, command, &auction, proxy_plans).await
        }
    }
}

async fn apply_preview_plan_sqlite(
    transaction: &mut SqliteCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    manual: bool,
    successor: &AuctionPreviewSuccessorPlan,
) -> Result<(), CompetitionTransactionError> {
    let world_id: String = {
        let connection = transaction.connection_mut()?;
        let row = sqlx::query(
            "SELECT auction.world_id, world.enabled, auction.predecessor_auction_id
             FROM auctions AS auction
             INNER JOIN worlds AS world ON world.id = auction.world_id
             WHERE auction.id = ?",
        )
        .bind(uuid_blob(auction.id.as_uuid()))
        .fetch_optional(&mut *connection)
        .await
        .map_err(map_sql_error)?
        .ok_or(CompetitionTransactionError::NotFound)?;
        if row.try_get::<i64, _>("enabled").map_err(map_sql_error)? == 0 {
            return Err(CompetitionTransactionError::InvalidState);
        }
        if let Some(predecessor) =
            row.try_get::<Option<Vec<u8>>, _>("predecessor_auction_id").map_err(map_sql_error)?
        {
            let predecessor = sqlx::query("SELECT world_id, status FROM auctions WHERE id = ?")
                .bind(predecessor)
                .fetch_optional(&mut *connection)
                .await
                .map_err(map_sql_error)?
                .ok_or_else(|| invalid_state("Lite 预览拍卖的前驱记录不存在"))?;
            let predecessor_world: String =
                predecessor.try_get("world_id").map_err(map_sql_error)?;
            let predecessor_status: String =
                predecessor.try_get("status").map_err(map_sql_error)?;
            let current_world: String = row.try_get("world_id").map_err(map_sql_error)?;
            if predecessor_world != current_world
                || !matches!(predecessor_status.as_str(), "settled" | "cancelled")
            {
                return Err(CompetitionTransactionError::InvalidState);
            }
        }
        row.try_get("world_id").map_err(map_sql_error)?
    };
    let now = sqlite_timestamp(auction.updated_at);
    let preview_revision = {
        let connection = transaction.connection_mut()?;
        sqlx::query_scalar::<_, i64>(
            "UPDATE auctions
             SET status = 'preview', preview_opened_at = COALESCE(preview_opened_at, ?),
                 preview_manual_open = CASE WHEN preview_manual_open = 1 OR ? THEN 1 ELSE 0 END,
                 preview_revision = preview_revision + 1, state_version = ?, updated_at = ?
             WHERE id = ? AND status = 'draft' AND state_version = ?
               AND (? OR preview_scheduled_at <= ?)
             RETURNING preview_revision",
        )
        .bind(&now)
        .bind(manual)
        .bind(i64_version(auction.state_version, "Lite 预览状态版本")?)
        .bind(&now)
        .bind(uuid_blob(auction.id.as_uuid()))
        .bind(previous_lifecycle_version_sqlite(auction)?)
        .bind(manual)
        .bind(&now)
        .fetch_optional(&mut *connection)
        .await
        .map_err(map_sql_error)?
    };
    if preview_revision.is_none_or(|value| value <= 0) {
        return Err(CompetitionTransactionError::InvalidState);
    }
    let materialized = {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "INSERT INTO auction_preview_items
                (auction_id, equipment_id, display_order, start_price, source_revision, collected_at)
             SELECT lot.auction_id, lot.equipment_id, lot.lot_number, lot.start_price,
                    auction.preview_revision, equipment.collected_at
             FROM lots AS lot
             INNER JOIN auctions AS auction ON auction.id = lot.auction_id
             INNER JOIN equipment AS equipment ON equipment.id = lot.equipment_id
             WHERE lot.auction_id = ? AND lot.status = 'pending' AND auction.status = 'preview'
             ORDER BY lot.lot_number",
        )
        .bind(uuid_blob(auction.id.as_uuid()))
        .execute(&mut *connection)
        .await
        .map_err(map_sql_error)?
        .rows_affected()
    };
    let pending_count: i64 = {
        let connection = transaction.connection_mut()?;
        sqlx::query_scalar("SELECT COUNT(*) FROM lots WHERE auction_id = ? AND status = 'pending'")
            .bind(uuid_blob(auction.id.as_uuid()))
            .fetch_one(&mut *connection)
            .await
            .map_err(map_sql_error)?
    };
    if i64::try_from(materialized).ok() != Some(pending_count) {
        return Err(invalid_state("Lite 预览物化条目数与 pending lot 不一致"));
    }

    let successor_version = {
        let connection = transaction.connection_mut()?;
        sqlx::query_scalar::<_, i64>(
            "INSERT INTO auctions
                (id, world_id, sequence, title, status, default_start_price,
                 soft_close_window_seconds, soft_close_extension_seconds,
                 cancel_cutoff_seconds, proxy_slow_interval_seconds, policy_snapshot,
                 preview_scheduled_at, scheduled_open_at, predecessor_auction_id,
                 state_version, created_at, updated_at)
             SELECT ?, current.world_id, ?, ?, 'draft', current.default_start_price,
                    current.soft_close_window_seconds, current.soft_close_extension_seconds,
                    current.cancel_cutoff_seconds, current.proxy_slow_interval_seconds,
                    json_set(current.policy_snapshot, '$.scheduled_end_at', ?),
                    ?, ?, current.id, 1, ?, ?
             FROM auctions AS current
             WHERE current.id = ? AND current.status = 'preview'
             RETURNING state_version",
        )
        .bind(uuid_blob(successor.auction_id.as_uuid()))
        .bind(successor.sequence)
        .bind(&successor.title)
        .bind(successor.scheduled_end_at.to_rfc3339())
        .bind(sqlite_timestamp(successor.preview_due_at))
        .bind(sqlite_timestamp(successor.open_due_at))
        .bind(&now)
        .bind(&now)
        .bind(uuid_blob(auction.id.as_uuid()))
        .fetch_optional(&mut *connection)
        .await
        .map_err(map_sql_error)?
    };
    if successor_version != Some(1) {
        return Err(invalid_state("Lite 下一期草稿不满足初始状态不变量"));
    }

    transaction
        .insert_command_inner(StoredCommand {
            command_id: successor.create_command_id,
            command_kind: "scheduled_auction_create".to_owned(),
            aggregate_id: successor.auction_id.as_uuid(),
            request_hash: successor.create_request_hash,
            result_code: "accepted".to_owned(),
            result_payload: serde_json::to_vec(&json!({
                "command_id": successor.create_command_id.as_uuid(),
                "auction_id": successor.auction_id.as_uuid(),
                "status": "draft",
                "state_version": 1,
            }))
            .map_err(|_| invalid_record())?,
        })
        .await?;
    {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "INSERT INTO scheduled_rounds
                (id, world_id, local_week, auction_id, status, preview_due_at,
                 open_due_at, scheduled_end_at, create_command_id,
                 state_version, created_at, updated_at)
             VALUES (?, ?, ?, ?, 'active', ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(uuid_blob(successor.round_id))
        .bind(&world_id)
        .bind(successor.local_week.format("%Y-%m-%d").to_string())
        .bind(uuid_blob(successor.auction_id.as_uuid()))
        .bind(sqlite_timestamp(successor.preview_due_at))
        .bind(sqlite_timestamp(successor.open_due_at))
        .bind(sqlite_timestamp(successor.scheduled_end_at))
        .bind(uuid_blob(successor.create_command_id.as_uuid()))
        .bind(&now)
        .bind(&now)
        .execute(&mut *connection)
        .await
        .map_err(map_sql_error)?;
        if manual {
            sqlx::query(
                "UPDATE scheduled_rounds
                 SET preview_command_id = ?, preview_completed_at = ?,
                     state_version = state_version + 1, updated_at = ?
                 WHERE auction_id = ? AND status = 'active' AND preview_completed_at IS NULL",
            )
            .bind(uuid_blob(command.command_id.as_uuid()))
            .bind(&now)
            .bind(&now)
            .bind(uuid_blob(auction.id.as_uuid()))
            .execute(&mut *connection)
            .await
            .map_err(map_sql_error)?;
        }
    }
    Ok(())
}

async fn apply_open_plan_sqlite(
    transaction: &mut SqliteCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    manual_override: bool,
) -> Result<(), CompetitionTransactionError> {
    let scheduled_end_at =
        auction.scheduled_end_at.ok_or_else(|| invalid_state("Lite 开拍计划缺少名义结束时间"))?;
    if auction.effective_end_at != Some(scheduled_end_at) {
        return Err(invalid_state("Lite 开拍计划的名义与有效结束时间不一致"));
    }
    let lot_rows = {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "SELECT status, submission_id FROM lots WHERE auction_id = ? ORDER BY lot_number",
        )
        .bind(uuid_blob(auction.id.as_uuid()))
        .fetch_all(&mut *connection)
        .await
        .map_err(map_sql_error)?
    };
    if lot_rows.is_empty()
        || lot_rows.iter().any(|row| {
            row.try_get::<String, _>("status").ok().as_deref() != Some("pending")
                || row.try_get::<Option<Vec<u8>>, _>("submission_id").ok().flatten().is_none()
        })
    {
        return Err(CompetitionTransactionError::InvalidState);
    }
    let now = sqlite_timestamp(auction.updated_at);
    let activated = {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "UPDATE lots
             SET status = 'live', current_price = start_price, effective_end_at = ?,
                 slow_due_at = NULL, slow_authorization_id = NULL, slow_generation = 0,
                 last_proxy_sequence = 0, state_version = state_version + 1, updated_at = ?
             WHERE auction_id = ? AND status = 'pending'",
        )
        .bind(sqlite_timestamp(scheduled_end_at))
        .bind(&now)
        .bind(uuid_blob(auction.id.as_uuid()))
        .execute(&mut *connection)
        .await
        .map_err(map_sql_error)?
        .rows_affected()
    };
    if usize::try_from(activated).ok() != Some(lot_rows.len()) {
        return Err(CompetitionTransactionError::InvalidState);
    }
    let updated = {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "UPDATE auctions
             SET status = 'live', preview_opened_at = COALESCE(preview_opened_at, ?),
                 preview_scheduled_at = CASE
                     WHEN ? AND preview_scheduled_at > ? THEN ? ELSE preview_scheduled_at END,
                 scheduled_open_at = CASE WHEN ? THEN ? ELSE scheduled_open_at END,
                 opened_at = ?, scheduled_end_at = ?, effective_end_at = ?,
                 state_version = ?, updated_at = ?
             WHERE id = ? AND status = 'preview' AND state_version = ?
               AND (preview_scheduled_at <= ? OR ?)
               AND (scheduled_open_at <= ? OR ?)",
        )
        .bind(&now)
        .bind(manual_override)
        .bind(&now)
        .bind(&now)
        .bind(manual_override)
        .bind(&now)
        .bind(&now)
        .bind(sqlite_timestamp(scheduled_end_at))
        .bind(sqlite_timestamp(scheduled_end_at))
        .bind(i64_version(auction.state_version, "Lite 开拍状态版本")?)
        .bind(&now)
        .bind(uuid_blob(auction.id.as_uuid()))
        .bind(previous_lifecycle_version_sqlite(auction)?)
        .bind(&now)
        .bind(manual_override)
        .bind(&now)
        .bind(manual_override)
        .execute(&mut *connection)
        .await
        .map_err(map_sql_error)?
    };
    if updated.rows_affected() != 1 {
        return Err(CompetitionTransactionError::InvalidState);
    }
    if command.actor.actor_type == auction_application::AuctionLifecycleActorType::Admin {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "UPDATE scheduled_rounds
             SET open_command_id = ?, open_completed_at = ?,
                 state_version = state_version + 1, updated_at = ?
             WHERE auction_id = ? AND status = 'active'
               AND preview_completed_at IS NOT NULL AND open_completed_at IS NULL",
        )
        .bind(uuid_blob(command.command_id.as_uuid()))
        .bind(&now)
        .bind(&now)
        .bind(uuid_blob(auction.id.as_uuid()))
        .execute(&mut *connection)
        .await
        .map_err(map_sql_error)?;
    }
    Ok(())
}

async fn apply_cancel_plan_sqlite(
    transaction: &mut SqliteCompetitionTransaction,
    auction: &Auction,
) -> Result<(), CompetitionTransactionError> {
    let previous_status: String = {
        let connection = transaction.connection_mut()?;
        sqlx::query_scalar("SELECT status FROM auctions WHERE id = ?")
            .bind(uuid_blob(auction.id.as_uuid()))
            .fetch_optional(&mut *connection)
            .await
            .map_err(map_sql_error)?
            .ok_or(CompetitionTransactionError::NotFound)?
    };
    if !matches!(previous_status.as_str(), "draft" | "preview") {
        return Err(CompetitionTransactionError::InvalidState);
    }
    let now = sqlite_timestamp(auction.updated_at);
    let connection = transaction.connection_mut()?;
    sqlx::query(
        "UPDATE lots
         SET status = 'withdrawn', slow_authorization_id = NULL, slow_due_at = NULL,
             state_version = state_version + 1, updated_at = ?
         WHERE auction_id = ? AND status = 'pending'",
    )
    .bind(&now)
    .bind(uuid_blob(auction.id.as_uuid()))
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        "UPDATE scheduled_rounds
         SET status = 'missed', auction_id = NULL, lease_owner = NULL, lease_until = NULL,
             state_version = state_version + 1, updated_at = ?
         WHERE auction_id = ? AND status = 'active' AND open_completed_at IS NULL",
    )
    .bind(&now)
    .bind(uuid_blob(auction.id.as_uuid()))
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    let updated = sqlx::query(
        "UPDATE auctions
         SET status = 'cancelled', state_version = ?, updated_at = ?,
             cancelled_at = ?, cancelled_reason = ?, cancelled_by = ?
         WHERE id = ? AND status = ? AND state_version = ?",
    )
    .bind(i64_version(auction.state_version, "Lite 取消状态版本")?)
    .bind(&now)
    .bind(auction.cancelled_at.map(sqlite_timestamp))
    .bind(&auction.cancelled_reason)
    .bind(&auction.cancelled_by)
    .bind(uuid_blob(auction.id.as_uuid()))
    .bind(previous_status)
    .bind(previous_lifecycle_version_sqlite(auction)?)
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    if updated.rows_affected() != 1 {
        return Err(CompetitionTransactionError::InvalidState);
    }
    Ok(())
}

async fn apply_final_plan_sqlite(
    transaction: &mut SqliteCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    proxy_plans: Vec<auction_application::AuctionFinalProxyPlan>,
) -> Result<(), CompetitionTransactionError> {
    let now = sqlite_timestamp(auction.updated_at);
    let updated = {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "UPDATE auctions
             SET status = 'final', final_phase_at = ?, state_version = ?, updated_at = ?
             WHERE id = ? AND status = 'live' AND state_version = ?",
        )
        .bind(&now)
        .bind(i64_version(auction.state_version, "Lite 最终阶段状态版本")?)
        .bind(&now)
        .bind(uuid_blob(auction.id.as_uuid()))
        .bind(previous_lifecycle_version_sqlite(auction)?)
        .execute(&mut *connection)
        .await
        .map_err(map_sql_error)?
    };
    if updated.rows_affected() != 1 {
        return Err(CompetitionTransactionError::InvalidState);
    }
    for proxy_plan in proxy_plans {
        if proxy_plan.command.aggregate_id != proxy_plan.competition.final_lot.lot_id.as_uuid() {
            return Err(invalid_state("Lite 最终阶段代理子命令与 lot 计划不一致"));
        }
        transaction.insert_command_inner(proxy_plan.command).await?;
        transaction.apply_plan_inner(proxy_plan.competition, command.request_id.clone()).await?;
    }
    if command.actor.actor_type == auction_application::AuctionLifecycleActorType::Admin {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "UPDATE scheduled_rounds
             SET final_command_id = ?, final_completed_at = ?,
                 lease_owner = NULL, lease_until = NULL,
                 state_version = state_version + 1, updated_at = ?
             WHERE auction_id = ? AND status = 'active'
               AND open_completed_at IS NOT NULL AND final_completed_at IS NULL",
        )
        .bind(uuid_blob(command.command_id.as_uuid()))
        .bind(&now)
        .bind(&now)
        .bind(uuid_blob(auction.id.as_uuid()))
        .execute(&mut *connection)
        .await
        .map_err(map_sql_error)?;
    }
    Ok(())
}

fn previous_lifecycle_version_sqlite(
    auction: &Auction,
) -> Result<i64, CompetitionTransactionError> {
    auction
        .state_version
        .checked_sub(1)
        .and_then(|version| i64::try_from(version).ok())
        .ok_or_else(|| invalid_state("Lite 拍卖状态版本无法回退校验"))
}

async fn append_lifecycle_record_inner(
    transaction: &mut SqliteCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    result: &AuctionLifecycleResult,
) -> Result<(), CompetitionTransactionError> {
    let metadata = serde_json::to_string(&json!({
        "command_id": command.command_id.as_uuid(),
        "action": command.action.command_kind(),
        "status": auction_status_name(result.status),
        "state_version": result.state_version,
    }))
    .map_err(|_| invalid_record())?;
    {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "INSERT INTO audit_records
                (id, actor_type, actor_id, action, entity_type, entity_id, result, request_id, source, metadata)
             VALUES (?, ?, ?, ?, 'auction', ?, 'success', ?, ?, ?)",
        )
        .bind(uuid_blob(Uuid::new_v4()))
        .bind(command.actor.actor_type.database_name())
        .bind(&command.actor.actor_id)
        .bind(command.action.command_kind())
        .bind(result.auction_id.as_uuid().to_string())
        .bind(command.request_id.as_deref())
        .bind(command.actor.actor_type.database_name())
        .bind(metadata)
        .execute(&mut *connection)
        .await
        .map_err(map_sql_error)?;
    }

    let (event_kind, payload) =
        lifecycle_event_payload(transaction.connection_mut()?, command, result).await?;
    let payload = serde_json::to_string(&payload).map_err(|_| invalid_record())?;
    let stream_key = format!("auction:{}", result.auction_id.as_uuid());
    let connection = transaction.connection_mut()?;
    sqlx::query(
        "INSERT INTO realtime_streams (stream_key, next_sequence, updated_at)
         VALUES (?, 1, CURRENT_TIMESTAMP)
         ON CONFLICT(stream_key) DO NOTHING",
    )
    .bind(&stream_key)
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    let sequence: i64 =
        sqlx::query_scalar("SELECT next_sequence FROM realtime_streams WHERE stream_key = ?")
            .bind(&stream_key)
            .fetch_one(&mut *connection)
            .await
            .map_err(map_sql_error)?;
    sqlx::query(
        "UPDATE realtime_streams
         SET next_sequence = next_sequence + 1, updated_at = CURRENT_TIMESTAMP
         WHERE stream_key = ?",
    )
    .bind(&stream_key)
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        "INSERT INTO realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
             state_version, origin_command_id)
         VALUES (?, ?, ?, ?, 1, ?, ?, ?)",
    )
    .bind(uuid_blob(Uuid::new_v4()))
    .bind(&stream_key)
    .bind(sequence)
    .bind(event_kind)
    .bind(payload)
    .bind(i64_version(result.state_version, "生命周期事件状态版本")?)
    .bind(uuid_blob(command.command_id.as_uuid()))
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

async fn lifecycle_event_payload(
    connection: &mut SqliteConnection,
    command: &AuctionLifecycleCommand,
    result: &AuctionLifecycleResult,
) -> Result<(&'static str, Value), CompetitionTransactionError> {
    let row = sqlx::query(
        "SELECT a.world_id,
                COALESCE((SELECT MAX(source_revision)
                          FROM auction_preview_items AS item
                          WHERE item.auction_id = a.id), 0) AS preview_revision,
                (SELECT COUNT(*)
                 FROM auction_preview_items AS item
                 WHERE item.auction_id = a.id) AS item_count
         FROM auctions AS a
         WHERE a.id = ?",
    )
    .bind(uuid_blob(result.auction_id.as_uuid()))
    .fetch_one(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    let world_id: String = row.try_get("world_id").map_err(map_sql_error)?;
    let preview_revision: i64 = row.try_get("preview_revision").map_err(map_sql_error)?;
    let item_count: i64 = row.try_get("item_count").map_err(map_sql_error)?;
    let state_version = result.state_version;
    let event = match command.action {
        auction_application::AuctionLifecycleAction::Preview => (
            "auction.preview_updated",
            json!({
                "auction_id": result.auction_id.as_uuid(),
                "preview_revision": preview_revision,
                "item_count": item_count,
                "manual_open": false,
                "state_version": state_version,
            }),
        ),
        auction_application::AuctionLifecycleAction::Open => (
            "auction.opened",
            json!({
                "auction_id": result.auction_id.as_uuid(),
                "world_id": world_id,
                "state_version": state_version,
                "preview_revision": preview_revision,
            }),
        ),
        auction_application::AuctionLifecycleAction::Cancel => (
            "auction.cancelled",
            json!({ "auction_id": result.auction_id.as_uuid(), "state_version": state_version }),
        ),
        auction_application::AuctionLifecycleAction::FinalPhase => (
            "auction.final_phase_entered",
            json!({ "auction_id": result.auction_id.as_uuid(), "state_version": state_version }),
        ),
    };
    Ok(event)
}

impl SqliteCompetitionTransaction {
    async fn find_command_inner(
        &mut self,
        command_id: auction_domain::CommandId,
    ) -> Result<Option<StoredCommand>, CompetitionTransactionError> {
        let row = {
            let connection = self.connection_mut()?;
            sqlx::query(
                "SELECT command_kind, aggregate_id, request_hash, result_code, result_payload
                 FROM auction_commands WHERE command_id = ?",
            )
            .bind(uuid_blob(command_id.as_uuid()))
            .fetch_optional(&mut *connection)
            .await
            .map_err(map_sql_error)?
        };
        let Some(row) = row else {
            return Ok(None);
        };

        let request_hash: [u8; 32] = row
            .try_get::<Vec<u8>, _>("request_hash")
            .map_err(map_sql_error)?
            .try_into()
            .map_err(|_| invalid_record())?;
        let result_payload: String = row.try_get("result_payload").map_err(map_sql_error)?;
        let payload: Value = serde_json::from_str(&result_payload).map_err(|_| invalid_record())?;
        let stored = StoredCommand {
            command_id,
            command_kind: row.try_get("command_kind").map_err(map_sql_error)?,
            aggregate_id: uuid_from_blob(
                row.try_get("aggregate_id").map_err(map_sql_error)?,
                "命令聚合标识",
            )?,
            request_hash: RequestHash::new(request_hash),
            result_code: row.try_get("result_code").map_err(map_sql_error)?,
            result_payload: serde_json::to_vec(&payload).map_err(|_| invalid_record())?,
        };
        stored.validate().map_err(CompetitionTransactionError::from)?;
        Ok(Some(stored))
    }

    async fn insert_command_inner(
        &mut self,
        command: StoredCommand,
    ) -> Result<(), CompetitionTransactionError> {
        command.validate().map_err(CompetitionTransactionError::from)?;
        let payload: Value =
            serde_json::from_slice(&command.result_payload).map_err(|_| invalid_record())?;
        let payload = serde_json::to_string(&payload).map_err(|_| invalid_record())?;
        let inserted = {
            let connection = self.connection_mut()?;
            sqlx::query(
                "INSERT INTO auction_commands
                    (command_id, command_kind, aggregate_id, request_hash, result_code, result_payload)
                 VALUES (?, ?, ?, ?, ?, ?)
                 ON CONFLICT(command_id) DO NOTHING",
            )
            .bind(uuid_blob(command.command_id.as_uuid()))
            .bind(&command.command_kind)
            .bind(uuid_blob(command.aggregate_id))
            .bind(command.request_hash.as_bytes().to_vec())
            .bind(&command.result_code)
            .bind(payload)
            .execute(&mut *connection)
            .await
            .map_err(map_sql_error)?
        };

        if inserted.rows_affected() == 1 {
            return Ok(());
        }
        let Some(existing) = self.find_command_inner(command.command_id).await? else {
            return Err(CompetitionTransactionError::Failed(
                "命令冲突后无法读取已有账本记录".to_owned(),
            ));
        };
        if existing == command {
            Ok(())
        } else {
            Err(CompetitionTransactionError::CommandLedger(CommandLedgerError::Conflict))
        }
    }

    async fn load_for_update_inner(
        &mut self,
        lot_id: LotId,
    ) -> Result<CompetitionSnapshot, CompetitionTransactionError> {
        let row = {
            let connection = self.connection_mut()?;
            sqlx::query(
                "SELECT
                    l.start_price, l.current_price, l.leader_user_id, l.leader_proxy_last,
                    l.effective_end_at, l.state_version, l.last_proxy_sequence,
                    l.slow_generation, l.slow_authorization_id, l.slow_due_at, l.material,
                    a.default_start_price, a.soft_close_window_seconds,
                    a.soft_close_extension_seconds, a.cancel_cutoff_seconds,
                    a.proxy_slow_interval_seconds,
                    COALESCE(CAST(json_extract(a.policy_snapshot, '$.bid_increment_ppm') AS INTEGER), 0)
                        AS policy_bid_increment_ppm,
                    COALESCE(CAST(json_extract(a.policy_snapshot, '$.minimum_increment') AS INTEGER), 30000)
                        AS policy_minimum_increment,
                    COALESCE(CAST(json_extract(a.policy_snapshot, '$.proxy_enabled') AS INTEGER), 1)
                        AS policy_proxy_enabled,
                    COALESCE(json_extract(a.policy_snapshot, '$.proxy_default_mode'), 'fast')
                        AS policy_proxy_default_mode,
                    COALESCE(CAST(json_extract(a.policy_snapshot, '$.proxy_max_outbid_uses') AS INTEGER), 0)
                        AS policy_proxy_max_outbid_uses,
                    COALESCE(CAST(json_extract(a.policy_snapshot, '$.final_phase_lead_seconds') AS INTEGER), 60)
                        AS policy_final_phase_lead_seconds
                 FROM lots AS l
                 INNER JOIN auctions AS a ON a.id = l.auction_id
                 WHERE l.id = ? AND l.status = 'live'",
            )
            .bind(uuid_blob(lot_id.as_uuid()))
            .fetch_optional(&mut *connection)
            .await
            .map_err(map_sql_error)?
        };
        let Some(row) = row else {
            return Err(CompetitionTransactionError::NotFound);
        };

        let rules = auction_rules_from_row(&row)?;
        let state = LotBidState {
            lot_id,
            start_price: bid_amount(&row, "start_price", "lot 起始价")?,
            current_price: optional_bid_amount(&row, "current_price", "lot 当前价")?,
            leader_id: row
                .try_get::<Option<Vec<u8>>, _>("leader_user_id")
                .map_err(map_sql_error)?
                .map(|bytes| uuid_from_blob(bytes, "领先者标识"))
                .transpose()?
                .map(auction_domain::UserId::from_uuid),
            leader_proxy_last: row.try_get::<i64, _>("leader_proxy_last").map_err(map_sql_error)?
                != 0,
            material: row.try_get::<i64, _>("material").map_err(map_sql_error)? != 0,
            effective_end_at: parse_timestamp(
                row.try_get::<Option<String>, _>("effective_end_at")
                    .map_err(map_sql_error)?
                    .ok_or_else(|| invalid_state("lot 缺少有效结束时间"))?,
                "lot 有效结束时间",
            )?,
            state_version: positive_u64(
                row.try_get("state_version").map_err(map_sql_error)?,
                "lot 状态版本",
            )?,
            last_proxy_sequence: nonnegative_u64(
                row.try_get("last_proxy_sequence").map_err(map_sql_error)?,
                "代理序号",
            )?,
            slow_generation: nonnegative_u64(
                row.try_get("slow_generation").map_err(map_sql_error)?,
                "慢速代理世代",
            )?,
            slow_authorization_id: row
                .try_get::<Option<Vec<u8>>, _>("slow_authorization_id")
                .map_err(map_sql_error)?
                .map(|bytes| uuid_from_blob(bytes, "慢速代理授权标识"))
                .transpose()?
                .map(ProxyAuthorizationId::from_uuid),
            slow_due_at: row
                .try_get::<Option<String>, _>("slow_due_at")
                .map_err(map_sql_error)?
                .map(|value| parse_timestamp(value, "慢速代理到期时间"))
                .transpose()?,
        };
        let active_proxy = {
            let connection = self.connection_mut()?;
            let row = sqlx::query(
                "SELECT authorization_id, lot_id, user_id, creation_command_id,
                    authorization_sequence, usage_number, maximum_amount, mode, status, state_version
                 FROM bid_proxy_authorizations
                 WHERE lot_id = ? AND status IN ('active', 'final_fast')
                 ORDER BY authorization_sequence DESC
                 LIMIT 1",
            )
            .bind(uuid_blob(lot_id.as_uuid()))
            .fetch_optional(&mut *connection)
            .await
            .map_err(map_sql_error)?;
            row.map(proxy_from_row).transpose()?
        };

        rules.validate().map_err(|error| invalid_state(error.to_string().as_str()))?;
        state.validate().map_err(|error| invalid_state(error.to_string().as_str()))?;
        Ok(CompetitionSnapshot { rules, state, active_proxy })
    }

    /// SQLite 事务已经通过 `BEGIN IMMEDIATE` 串行化；在事务内读取历史最大使用次数。
    async fn next_proxy_usage_number_inner(
        &mut self,
        lot_id: LotId,
        user_id: UserId,
    ) -> Result<u64, CompetitionTransactionError> {
        let next_usage: i64 = {
            let connection = self.connection_mut()?;
            sqlx::query_scalar(
                "SELECT COALESCE(MAX(usage_number), 0) + 1
                 FROM bid_proxy_authorizations
                 WHERE lot_id = ? AND user_id = ?",
            )
            .bind(uuid_blob(lot_id.as_uuid()))
            .bind(uuid_blob(user_id.as_uuid()))
            .fetch_one(&mut *connection)
            .await
            .map_err(map_sql_error)?
        };
        if next_usage <= 0 {
            return Err(invalid_state("代理使用次数超出数据库范围"));
        }
        u64::try_from(next_usage).map_err(|_| invalid_state("代理使用次数超出领域范围"))
    }

    async fn apply_plan_inner(
        &mut self,
        plan: CompetitionPlan,
        request_id: Option<String>,
    ) -> Result<(), CompetitionTransactionError> {
        plan.validate().map_err(|error| invalid_state(error.to_string().as_str()))?;
        let command_id = plan_command_id(&plan)?;
        apply_proxy_changes(self, &plan).await?;
        for bid in &plan.public_bids {
            insert_bid(self, bid).await?;
            insert_audit_for_bid(self, bid, request_id.as_deref()).await?;
            insert_realtime_event(self, bid).await?;
        }
        insert_audit_for_proxy_changes(self, &plan, command_id, request_id.as_deref()).await?;
        update_lot(self, &plan).await
    }
}

async fn apply_proxy_changes(
    transaction: &mut SqliteCompetitionTransaction,
    plan: &CompetitionPlan,
) -> Result<(), CompetitionTransactionError> {
    let created_ids: HashSet<ProxyAuthorizationId> = plan
        .proxy_transitions
        .iter()
        .filter(|transition| {
            matches!(transition.kind, auction_domain::ProxyTransitionKind::Created)
        })
        .map(|transition| transition.authorization_id)
        .collect();

    for state in &plan.proxy_states {
        if created_ids.contains(&state.id) {
            continue;
        }
        let previous_version = state
            .state_version
            .checked_sub(1)
            .ok_or_else(|| invalid_state("代理快照缺少前一版本"))?;
        let reason = terminal_reason(plan, state.id);
        let connection = transaction.connection_mut()?;
        let result = sqlx::query(
            "UPDATE bid_proxy_authorizations
             SET maximum_amount = ?, mode = ?, status = ?, terminal_reason = ?,
                 state_version = ?, updated_at = CURRENT_TIMESTAMP
             WHERE authorization_id = ? AND state_version = ?",
        )
        .bind(state.maximum_amount.value())
        .bind(proxy_mode(state.mode))
        .bind(proxy_status(state.status))
        .bind(reason.map(proxy_terminal_reason))
        .bind(i64_version(state.state_version, "代理状态版本")?)
        .bind(uuid_blob(state.id.as_uuid()))
        .bind(i64_version(previous_version, "代理前一版本")?)
        .execute(&mut *connection)
        .await
        .map_err(map_sql_error)?;
        if result.rows_affected() != 1 {
            return Err(CompetitionTransactionError::Failed(
                "代理快照版本与数据库不一致".to_owned(),
            ));
        }
    }

    for state in &plan.proxy_states {
        if !created_ids.contains(&state.id) {
            continue;
        }
        let reason = terminal_reason(plan, state.id);
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "INSERT INTO bid_proxy_authorizations
                (authorization_id, lot_id, user_id, authorization_sequence, usage_number,
                 maximum_amount, mode, status, terminal_reason, state_version,
                 creation_command_id, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
        )
        .bind(uuid_blob(state.id.as_uuid()))
        .bind(uuid_blob(state.lot_id.as_uuid()))
        .bind(uuid_blob(state.user_id.as_uuid()))
        .bind(i64_version(state.authorization_sequence, "代理序号")?)
        .bind(i64_version(state.usage_number, "代理使用次数")?)
        .bind(state.maximum_amount.value())
        .bind(proxy_mode(state.mode))
        .bind(proxy_status(state.status))
        .bind(reason.map(proxy_terminal_reason))
        .bind(i64_version(state.state_version, "代理状态版本")?)
        .bind(uuid_blob(state.creation_command_id.as_uuid()))
        .execute(&mut *connection)
        .await
        .map_err(map_sql_error)?;
    }

    for transition in &plan.proxy_transitions {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "INSERT INTO bid_proxy_authorization_transitions
                (authorization_id, transition_number, transition_kind, origin_command_id,
                 old_maximum_amount, new_maximum_amount, old_mode, new_mode, old_status,
                 new_status, reason, resulting_state_version, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)",
        )
        .bind(uuid_blob(transition.authorization_id.as_uuid()))
        .bind(i64_version(transition.state_version, "代理转换序号")?)
        .bind(proxy_transition_kind(transition.kind))
        .bind(uuid_blob(transition.command_id.as_uuid()))
        .bind(transition.old_maximum.map(BidAmount::value))
        .bind(transition.new_maximum.value())
        .bind(transition.old_mode.map(proxy_mode))
        .bind(proxy_mode(transition.new_mode))
        .bind(transition.old_status.map(proxy_status))
        .bind(proxy_status(transition.new_status))
        .bind(transition.reason.map(proxy_terminal_reason))
        .bind(i64_version(transition.state_version, "代理转换版本")?)
        .execute(&mut *connection)
        .await
        .map_err(map_sql_error)?;
    }
    Ok(())
}

async fn insert_bid(
    transaction: &mut SqliteCompetitionTransaction,
    bid: &AcceptedBid,
) -> Result<(), CompetitionTransactionError> {
    let connection = transaction.connection_mut()?;
    sqlx::query(
        "INSERT INTO bids
            (id, command_id, fact_index, lot_id, bidder_user_id, amount, mechanism,
             effective_end_at, accepted_state_version, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)",
    )
    .bind(uuid_blob(bid.id.as_uuid()))
    .bind(uuid_blob(bid.command_id.as_uuid()))
    .bind(i32_version(bid.fact_index, "竞价事实序号")?)
    .bind(uuid_blob(bid.lot_id.as_uuid()))
    .bind(uuid_blob(bid.bidder_id.as_uuid()))
    .bind(bid.amount.value())
    .bind(bid_mechanism(bid.mechanism))
    .bind(bid.effective_end_at.to_rfc3339())
    .bind(i64_version(bid.state_version, "竞价状态版本")?)
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

async fn update_lot(
    transaction: &mut SqliteCompetitionTransaction,
    plan: &CompetitionPlan,
) -> Result<(), CompetitionTransactionError> {
    let previous_version = plan
        .final_lot
        .state_version
        .checked_sub(plan.public_bids.len() as u64)
        .ok_or_else(|| invalid_state("lot 状态版本回退失败"))?;
    let connection = transaction.connection_mut()?;
    let result = sqlx::query(
        "UPDATE lots
         SET current_price = ?,
             leader_since = CASE
                 WHEN leader_user_id IS NOT ? THEN CURRENT_TIMESTAMP
                 WHEN ? IS NULL THEN NULL
                 ELSE leader_since
             END,
             leader_user_id = ?, leader_proxy_last = ?, effective_end_at = ?,
             slow_authorization_id = ?, slow_due_at = ?, slow_generation = ?,
             last_proxy_sequence = ?, state_version = ?, updated_at = CURRENT_TIMESTAMP
         WHERE id = ? AND state_version = ?",
    )
    .bind(plan.final_lot.current_price.map(BidAmount::value))
    .bind(plan.final_lot.leader_id.map(|id| uuid_blob(id.as_uuid())))
    .bind(plan.final_lot.leader_id.map(|id| uuid_blob(id.as_uuid())))
    .bind(plan.final_lot.leader_id.map(|id| uuid_blob(id.as_uuid())))
    .bind(if plan.final_lot.leader_proxy_last { 1_i64 } else { 0_i64 })
    .bind(plan.final_lot.effective_end_at.to_rfc3339())
    .bind(plan.final_lot.slow_authorization_id.map(|id| uuid_blob(id.as_uuid())))
    .bind(plan.final_lot.slow_due_at.map(|value| value.to_rfc3339()))
    .bind(i64_version(plan.final_lot.slow_generation, "慢速代理世代")?)
    .bind(i64_version(plan.final_lot.last_proxy_sequence, "代理序号")?)
    .bind(i64_version(plan.final_lot.state_version, "lot 状态版本")?)
    .bind(uuid_blob(plan.final_lot.lot_id.as_uuid()))
    .bind(i64_version(previous_version, "lot 前一版本")?)
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    if result.rows_affected() != 1 {
        return Err(CompetitionTransactionError::Failed("lot 状态版本与数据库不一致".to_owned()));
    }
    Ok(())
}

async fn insert_audit_for_bid(
    transaction: &mut SqliteCompetitionTransaction,
    bid: &AcceptedBid,
    request_id: Option<&str>,
) -> Result<(), CompetitionTransactionError> {
    let metadata = json!({
        "bid_id": bid.id.as_uuid(),
        "command_id": bid.command_id.as_uuid(),
        "amount": bid.amount.value(),
        "mechanism": bid_mechanism(bid.mechanism),
        "state_version": bid.state_version,
    });
    let connection = transaction.connection_mut()?;
    sqlx::query(
        "INSERT INTO audit_records
            (id, actor_type, actor_id, action, entity_type, entity_id, result, request_id, source, metadata)
         VALUES (?, 'public_user', ?, 'place_bid', 'lot', ?, 'success', ?, 'public', ?)",
    )
    .bind(uuid_blob(Uuid::new_v4()))
    .bind(bid.bidder_id.as_uuid().to_string())
    .bind(bid.lot_id.as_uuid().to_string())
    .bind(request_id)
    .bind(serde_json::to_string(&metadata).map_err(|_| invalid_record())?)
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

async fn insert_audit_for_proxy_changes(
    transaction: &mut SqliteCompetitionTransaction,
    plan: &CompetitionPlan,
    command_id: auction_domain::CommandId,
    request_id: Option<&str>,
) -> Result<(), CompetitionTransactionError> {
    for transition in &plan.proxy_transitions {
        let metadata = json!({
            "command_id": command_id.as_uuid(),
            "authorization_id": transition.authorization_id.as_uuid(),
            "transition_kind": proxy_transition_kind(transition.kind),
            "state_version": transition.state_version,
        });
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "INSERT INTO audit_records
                (id, actor_type, actor_id, action, entity_type, entity_id, result, request_id, source, metadata)
             VALUES (?, 'system', 'auction-application', 'proxy_transition',
                     'proxy_authorization', ?, 'success', ?, 'system', ?)",
        )
        .bind(uuid_blob(Uuid::new_v4()))
        .bind(transition.authorization_id.as_uuid().to_string())
        .bind(request_id)
        .bind(serde_json::to_string(&metadata).map_err(|_| invalid_record())?)
        .execute(&mut *connection)
        .await
        .map_err(map_sql_error)?;
    }
    Ok(())
}

async fn insert_realtime_event(
    transaction: &mut SqliteCompetitionTransaction,
    bid: &AcceptedBid,
) -> Result<(), CompetitionTransactionError> {
    let connection = transaction.connection_mut()?;
    let auction_id = uuid_from_blob(
        sqlx::query_scalar::<_, Vec<u8>>("SELECT auction_id FROM lots WHERE id = ?")
            .bind(uuid_blob(bid.lot_id.as_uuid()))
            .fetch_one(&mut *connection)
            .await
            .map_err(map_sql_error)?,
        "实时事件拍卖标识",
    )?;
    let stream_key = format!("auction:{auction_id}");
    let payload = serde_json::to_string(&json!({
        "auction_id": auction_id,
        "lot_id": bid.lot_id.as_uuid(),
        "bid_id": bid.id.as_uuid(),
        "amount": bid.amount.value(),
        "effective_end": bid.effective_end_at,
        "state_version": bid.state_version,
        "fact_index": bid.fact_index,
        "mechanism": bid_mechanism_name(bid.mechanism),
    }))
    .map_err(|_| invalid_record())?;
    sqlx::query(
        "INSERT INTO realtime_streams (stream_key, next_sequence, updated_at)
         VALUES (?, 1, CURRENT_TIMESTAMP)
         ON CONFLICT(stream_key) DO NOTHING",
    )
    .bind(&stream_key)
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    let sequence: i64 =
        sqlx::query_scalar("SELECT next_sequence FROM realtime_streams WHERE stream_key = ?")
            .bind(&stream_key)
            .fetch_one(&mut *connection)
            .await
            .map_err(map_sql_error)?;
    sqlx::query(
        "UPDATE realtime_streams SET next_sequence = next_sequence + 1,
         updated_at = CURRENT_TIMESTAMP WHERE stream_key = ?",
    )
    .bind(&stream_key)
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        "INSERT INTO realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
             state_version, origin_command_id, origin_bid_id, origin_fact_index, created_at)
         VALUES (?, ?, ?, 'lot.bid_accepted', 2, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)",
    )
    .bind(uuid_blob(Uuid::new_v4()))
    .bind(&stream_key)
    .bind(sequence)
    .bind(payload)
    .bind(i64_version(bid.state_version, "实时事件状态版本")?)
    .bind(uuid_blob(bid.command_id.as_uuid()))
    .bind(uuid_blob(bid.id.as_uuid()))
    .bind(i32_version(bid.fact_index, "实时事件事实序号")?)
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

fn bid_mechanism_name(mechanism: BidMechanism) -> &'static str {
    match mechanism {
        BidMechanism::Ordinary => "ordinary",
        BidMechanism::Proxy => "proxy",
    }
}

fn plan_command_id(
    plan: &CompetitionPlan,
) -> Result<auction_domain::CommandId, CompetitionTransactionError> {
    plan.public_bids
        .first()
        .map(|bid| bid.command_id)
        .or_else(|| plan.proxy_transitions.first().map(|transition| transition.command_id))
        .ok_or_else(|| invalid_state("竞价计划没有可审计的命令标识"))
}

fn terminal_reason(
    plan: &CompetitionPlan,
    authorization_id: ProxyAuthorizationId,
) -> Option<ProxyTerminalReason> {
    plan.proxy_transitions
        .iter()
        .rev()
        .find(|transition| transition.authorization_id == authorization_id)
        .and_then(|transition| transition.reason)
}

fn proxy_from_row(row: SqliteRow) -> Result<ProxyAuthorization, CompetitionTransactionError> {
    let mode = parse_proxy_mode(row.try_get::<String, _>("mode").map_err(map_sql_error)?)?;
    let status = parse_proxy_status(row.try_get::<String, _>("status").map_err(map_sql_error)?)?;
    let authorization = ProxyAuthorization {
        id: ProxyAuthorizationId::from_uuid(uuid_from_blob(
            row.try_get("authorization_id").map_err(map_sql_error)?,
            "代理授权标识",
        )?),
        lot_id: LotId::from_uuid(uuid_from_blob(
            row.try_get("lot_id").map_err(map_sql_error)?,
            "代理 lot 标识",
        )?),
        user_id: auction_domain::UserId::from_uuid(uuid_from_blob(
            row.try_get("user_id").map_err(map_sql_error)?,
            "代理用户标识",
        )?),
        creation_command_id: auction_domain::CommandId::from_uuid(uuid_from_blob(
            row.try_get("creation_command_id").map_err(map_sql_error)?,
            "代理创建命令标识",
        )?),
        authorization_sequence: positive_u64(
            row.try_get("authorization_sequence").map_err(map_sql_error)?,
            "代理序号",
        )?,
        usage_number: positive_u64(
            row.try_get("usage_number").map_err(map_sql_error)?,
            "代理使用次数",
        )?,
        maximum_amount: bid_amount(&row, "maximum_amount", "代理上限")?,
        mode,
        status,
        state_version: positive_u64(
            row.try_get("state_version").map_err(map_sql_error)?,
            "代理状态版本",
        )?,
    };
    authorization.validate().map_err(|error| invalid_state(error.to_string().as_str()))?;
    Ok(authorization)
}

fn bid_amount(
    row: &SqliteRow,
    column: &str,
    label: &str,
) -> Result<BidAmount, CompetitionTransactionError> {
    let value: i64 = row.try_get(column).map_err(map_sql_error)?;
    BidAmount::new(value).map_err(|_| invalid_state(label))
}

fn optional_bid_amount(
    row: &SqliteRow,
    column: &str,
    label: &str,
) -> Result<Option<BidAmount>, CompetitionTransactionError> {
    let value: Option<i64> = row.try_get(column).map_err(map_sql_error)?;
    value.map(|value| BidAmount::new(value).map_err(|_| invalid_state(label))).transpose()
}

fn auction_rules_from_row(row: &SqliteRow) -> Result<AuctionRules, CompetitionTransactionError> {
    let rules = AuctionRules {
        default_start_price: bid_amount(row, "default_start_price", "拍卖默认起始价")?,
        bid_increment_ppm: row.try_get("policy_bid_increment_ppm").map_err(map_sql_error)?,
        minimum_increment: row.try_get("policy_minimum_increment").map_err(map_sql_error)?,
        soft_close_window: duration(
            row.try_get("soft_close_window_seconds").map_err(map_sql_error)?,
            "软截止窗口",
        )?,
        soft_close_extension: duration(
            row.try_get("soft_close_extension_seconds").map_err(map_sql_error)?,
            "软截止延长",
        )?,
        cancel_cutoff: duration(
            row.try_get("cancel_cutoff_seconds").map_err(map_sql_error)?,
            "取消截止",
        )?,
        proxy_slow_interval: duration(
            row.try_get("proxy_slow_interval_seconds").map_err(map_sql_error)?,
            "慢速代理间隔",
        )?,
        proxy_enabled: row.try_get::<i64, _>("policy_proxy_enabled").map_err(map_sql_error)? != 0,
        proxy_default_mode: parse_proxy_mode(
            row.try_get("policy_proxy_default_mode").map_err(map_sql_error)?,
        )?,
        proxy_max_outbid_uses: policy_u32(
            row.try_get("policy_proxy_max_outbid_uses").map_err(map_sql_error)?,
            "代理重复进场次数",
        )?,
        final_phase_lead: duration(
            row.try_get("policy_final_phase_lead_seconds").map_err(map_sql_error)?,
            "最终阶段领先时间",
        )?,
    };
    rules.validate().map_err(|error| invalid_state(error.to_string().as_str()))?;
    Ok(rules)
}

fn duration(seconds: i64, label: &str) -> Result<Duration, CompetitionTransactionError> {
    Duration::try_seconds(seconds).ok_or_else(|| invalid_state(label))
}

fn positive_u64(value: i64, label: &str) -> Result<u64, CompetitionTransactionError> {
    if value <= 0 {
        return Err(invalid_state(label));
    }
    value.try_into().map_err(|_| invalid_state(label))
}

fn nonnegative_u64(value: i64, label: &str) -> Result<u64, CompetitionTransactionError> {
    if value < 0 {
        return Err(invalid_state(label));
    }
    value.try_into().map_err(|_| invalid_state(label))
}

fn policy_u32(value: i64, label: &str) -> Result<u32, CompetitionTransactionError> {
    if value < 0 {
        return Err(invalid_state(label));
    }
    value.try_into().map_err(|_| invalid_state(label))
}

fn i64_version(value: u64, label: &str) -> Result<i64, CompetitionTransactionError> {
    value.try_into().map_err(|_| invalid_state(label))
}

fn i32_version(value: u32, label: &str) -> Result<i32, CompetitionTransactionError> {
    value.try_into().map_err(|_| invalid_state(label))
}

fn parse_proxy_mode(value: String) -> Result<ProxyMode, CompetitionTransactionError> {
    match value.as_str() {
        "fast" => Ok(ProxyMode::Fast),
        "slow" => Ok(ProxyMode::Slow),
        _ => Err(invalid_state("数据库代理模式无效")),
    }
}

fn parse_proxy_status(value: String) -> Result<ProxyStatus, CompetitionTransactionError> {
    match value.as_str() {
        "active" => Ok(ProxyStatus::Active),
        "final_fast" => Ok(ProxyStatus::FinalFast),
        "exhausted" => Ok(ProxyStatus::Exhausted),
        "frozen" => Ok(ProxyStatus::Frozen),
        "closed" => Ok(ProxyStatus::Closed),
        _ => Err(invalid_state("数据库代理状态无效")),
    }
}

fn proxy_mode(mode: ProxyMode) -> &'static str {
    match mode {
        ProxyMode::Fast => "fast",
        ProxyMode::Slow => "slow",
    }
}

fn proxy_status(status: ProxyStatus) -> &'static str {
    match status {
        ProxyStatus::Active => "active",
        ProxyStatus::FinalFast => "final_fast",
        ProxyStatus::Exhausted => "exhausted",
        ProxyStatus::Frozen => "frozen",
        ProxyStatus::Closed => "closed",
    }
}

fn proxy_terminal_reason(reason: ProxyTerminalReason) -> &'static str {
    match reason {
        ProxyTerminalReason::StrictlyDefeated => "strictly_defeated",
        ProxyTerminalReason::RankedBelow => "ranked_below",
        ProxyTerminalReason::EligibilityLost => "eligibility_lost",
        ProxyTerminalReason::CompetitionClosed => "competition_closed",
    }
}

fn proxy_transition_kind(kind: auction_domain::ProxyTransitionKind) -> &'static str {
    match kind {
        auction_domain::ProxyTransitionKind::Created => "created",
        auction_domain::ProxyTransitionKind::Lowered => "lowered",
        auction_domain::ProxyTransitionKind::FinalFast => "final_fast",
        auction_domain::ProxyTransitionKind::Exhausted => "exhausted",
        auction_domain::ProxyTransitionKind::Frozen => "frozen",
        auction_domain::ProxyTransitionKind::Closed => "closed",
    }
}

fn bid_mechanism(mechanism: BidMechanism) -> &'static str {
    match mechanism {
        BidMechanism::Ordinary => "ordinary",
        BidMechanism::Proxy => "proxy",
    }
}

fn parse_auction_status(value: String) -> Result<AuctionStatus, CompetitionTransactionError> {
    match value.as_str() {
        "draft" => Ok(AuctionStatus::Draft),
        "preview" => Ok(AuctionStatus::Preview),
        "live" => Ok(AuctionStatus::Live),
        "final" => Ok(AuctionStatus::Final),
        "settling" => Ok(AuctionStatus::Settling),
        "settled" => Ok(AuctionStatus::Settled),
        "cancelled" => Ok(AuctionStatus::Cancelled),
        _ => Err(invalid_state("Lite 拍卖状态不在允许集合内")),
    }
}

fn auction_status_name(status: AuctionStatus) -> &'static str {
    match status {
        AuctionStatus::Draft => "draft",
        AuctionStatus::Preview => "preview",
        AuctionStatus::Live => "live",
        AuctionStatus::Final => "final",
        AuctionStatus::Settling => "settling",
        AuctionStatus::Settled => "settled",
        AuctionStatus::Cancelled => "cancelled",
    }
}

fn uuid_blob(uuid: Uuid) -> Vec<u8> {
    uuid.as_bytes().to_vec()
}

fn uuid_from_blob(bytes: Vec<u8>, label: &str) -> Result<Uuid, CompetitionTransactionError> {
    Uuid::from_slice(&bytes).map_err(|_| invalid_state(label))
}

fn parse_timestamp(
    value: String,
    label: &str,
) -> Result<DateTime<Utc>, CompetitionTransactionError> {
    if let Ok(value) = DateTime::parse_from_rfc3339(&value) {
        return Ok(value.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(value) = NaiveDateTime::parse_from_str(&value, format) {
            return Ok(DateTime::<Utc>::from_naive_utc_and_offset(value, Utc));
        }
    }
    Err(invalid_state(label))
}

fn parse_optional_timestamp(
    value: Option<String>,
    label: &str,
) -> Result<Option<DateTime<Utc>>, CompetitionTransactionError> {
    value.map(|value| parse_timestamp(value, label)).transpose()
}

fn sqlite_timestamp(value: DateTime<Utc>) -> String {
    value.naive_utc().format("%Y-%m-%d %H:%M:%S%.f").to_string()
}

fn map_sql_error(error: sqlx::Error) -> CompetitionTransactionError {
    if is_busy_error(&error) {
        CompetitionTransactionError::SerializationConflict
    } else {
        CompetitionTransactionError::Failed("SQLite 竞价事务操作失败".to_owned())
    }
}

fn is_busy_error(error: &sqlx::Error) -> bool {
    matches!(
        error,
        sqlx::Error::Database(database_error)
            if matches!(database_error.code().as_deref(), Some("5") | Some("6"))
    )
}

fn invalid_record() -> CompetitionTransactionError {
    CompetitionTransactionError::CommandLedger(CommandLedgerError::InvalidRecord)
}

fn invalid_state(message: &str) -> CompetitionTransactionError {
    CompetitionTransactionError::Failed(message.to_owned())
}
