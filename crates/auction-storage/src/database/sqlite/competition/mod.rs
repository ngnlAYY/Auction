//! SQLite Lite 档竞价事务。
//!
//! SQLite 没有 `FOR UPDATE`。整个写路径先取得连接并执行 `BEGIN IMMEDIATE`，
//! 由 SQLite 的单写者锁保护 lot、代理、命令账本和事件写入。

use std::{collections::HashSet, future::Future};

use auction_application::{
    AuctionLifecycleCommand, AuctionLifecycleResult, AuctionLifecycleTransaction,
    CommandLedgerError, CompetitionSnapshot, CompetitionTransaction, CompetitionTransactionError,
    RequestHash, StoredCommand, TransactionFailure, TransactionFuture, TransactionIsolation,
    TransactionPolicy, TransactionRunner,
};
use auction_domain::{
    AcceptedBid, Auction, AuctionId, AuctionRules, AuctionStatus, BidAmount, BidMechanism,
    CompetitionPlan, LotBidState, LotId, ProxyAuthorization, ProxyAuthorizationId, ProxyMode,
    ProxyStatus, ProxyTerminalReason, UserId,
};
use chrono::{DateTime, Duration, NaiveDateTime, Utc};
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
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        async move { self.apply_plan_inner(plan).await }
    }
}

#[allow(clippy::manual_async_fn)]
impl AuctionLifecycleTransaction for SqliteCompetitionTransaction {
    fn load_auction_for_update(
        &mut self,
        auction_id: AuctionId,
    ) -> impl Future<Output = Result<Option<Auction>, CompetitionTransactionError>> + Send {
        async move {
            let row = {
                let connection = self.connection_mut()?;
                sqlx::query(
                    "SELECT id, title, status, state_version, created_at, updated_at,
                            scheduled_end_at, effective_end_at
                     FROM auctions
                     WHERE id = ?",
                )
                .bind(uuid_blob(auction_id.as_uuid()))
                .fetch_optional(&mut *connection)
                .await
                .map_err(map_sql_error)?
            };
            let Some(row) = row else {
                return Ok(None);
            };
            let state_version =
                u64::try_from(row.try_get::<i64, _>("state_version").map_err(map_sql_error)?)
                    .map_err(|_| invalid_state("Lite 拍卖状态版本无效"))?;
            if state_version == 0 {
                return Err(invalid_state("Lite 拍卖状态版本无效"));
            }
            let id =
                uuid_from_blob(row.try_get("id").map_err(map_sql_error)?, "Lite 拍卖标识格式无效")?;
            Ok(Some(Auction {
                id: AuctionId::from_uuid(id),
                title: row.try_get("title").map_err(map_sql_error)?,
                status: parse_auction_status(
                    row.try_get::<String, _>("status").map_err(map_sql_error)?,
                )?,
                state_version,
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
            }))
        }
    }

    fn update_auction(
        &mut self,
        auction: Auction,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        async move {
            let previous_version = auction
                .state_version
                .checked_sub(1)
                .and_then(|version| i64::try_from(version).ok())
                .ok_or_else(|| invalid_state("Lite 拍卖状态版本无法回退校验"))?;
            let next_version = i64::try_from(auction.state_version)
                .map_err(|_| invalid_state("Lite 拍卖状态版本超出数据库范围"))?;
            let result = {
                let connection = self.connection_mut()?;
                sqlx::query(
                    "UPDATE auctions
                     SET status = ?,
                         state_version = ?,
                         updated_at = ?,
                         scheduled_end_at = ?,
                         effective_end_at = ?
                     WHERE id = ? AND state_version = ?",
                )
                .bind(auction_status_name(auction.status))
                .bind(next_version)
                .bind(sqlite_timestamp(auction.updated_at))
                .bind(auction.scheduled_end_at.map(sqlite_timestamp))
                .bind(auction.effective_end_at.map(sqlite_timestamp))
                .bind(uuid_blob(auction.id.as_uuid()))
                .bind(previous_version)
                .execute(&mut *connection)
                .await
                .map_err(map_sql_error)?
            };
            if result.rows_affected() != 1 {
                return Err(CompetitionTransactionError::NotFound);
            }
            Ok(())
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
        .bind(command.command_id.as_uuid().to_string())
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
                    a.proxy_slow_interval_seconds
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

        let rules = AuctionRules {
            default_start_price: bid_amount(&row, "default_start_price", "拍卖默认起始价")?,
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
        };
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
    ) -> Result<(), CompetitionTransactionError> {
        plan.validate().map_err(|error| invalid_state(error.to_string().as_str()))?;
        let command_id = plan_command_id(&plan)?;
        apply_proxy_changes(self, &plan).await?;
        for bid in &plan.public_bids {
            insert_bid(self, bid).await?;
            insert_audit_for_bid(self, bid).await?;
            insert_realtime_event(self, bid).await?;
        }
        insert_audit_for_proxy_changes(self, &plan, command_id).await?;
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
         SET current_price = ?, leader_user_id = ?, leader_proxy_last = ?, effective_end_at = ?,
             slow_authorization_id = ?, slow_due_at = ?, slow_generation = ?,
             last_proxy_sequence = ?, state_version = ?, updated_at = CURRENT_TIMESTAMP
         WHERE id = ? AND state_version = ?",
    )
    .bind(plan.final_lot.current_price.map(BidAmount::value))
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
    .bind(bid.command_id.as_uuid().to_string())
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
) -> Result<(), CompetitionTransactionError> {
    for transition in &plan.proxy_transitions {
        let metadata = json!({
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
        .bind(command_id.as_uuid().to_string())
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
