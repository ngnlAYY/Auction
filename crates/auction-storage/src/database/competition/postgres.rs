use std::{collections::HashSet, future::Future};

use auction_application::{
    AdminAuctionCreateTransaction, AdminAuctionSummary, AdminCreateAuctionCommand, AdminLotSummary,
    AdminLotUpdateCommand, AdminLotUpdateSnapshot, AdminLotUpdateTransaction,
    AuctionLifecycleCommand, AuctionLifecycleContext, AuctionLifecycleEffect, AuctionLifecyclePlan,
    AuctionLifecycleResult, AuctionLifecycleTransaction, AuctionPolicyTemplate,
    AuctionPreviewSuccessorPlan, CommandLedgerError, CompetitionSnapshot, CompetitionTransaction,
    CompetitionTransactionError, RequestHash, SettlementAuctionSnapshot, StoredCommand,
    TransactionFailure, TransactionFuture, TransactionIsolation, TransactionPolicy,
    TransactionRunner,
};
use auction_domain::{
    AcceptedBid, Auction, AuctionId, AuctionRules, AuctionStatus, BidAmount, BidMechanism,
    CompetitionPlan, LotBidState, LotId, ProxyAuthorization, ProxyAuthorizationId, ProxyMode,
    ProxyStatus, ProxyTerminalReason, UserId, deterministic_uuid,
};
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction, postgres::PgRow};
use uuid::Uuid;

use crate::database::audit_health::AuditWriteResult as _;

use super::PostgresStore;

type PgTransaction = Transaction<'static, Postgres>;

/// PostgreSQL 竞价事务的唯一实现。
///
/// 事务对象拥有 SQLx 从连接池取得的连接，因此应用回调结束后 runner 仍能提交或回滚；
/// 回调不能拿到独立连接，也不能把命令账本和竞价写入拆成两个事务。
pub struct PostgresCompetitionTransaction {
    transaction: Option<PgTransaction>,
}

impl PostgresCompetitionTransaction {
    pub(crate) fn new(transaction: PgTransaction) -> Self {
        Self { transaction: Some(transaction) }
    }

    pub(crate) async fn commit(mut self) -> Result<(), sqlx::Error> {
        match self.transaction.take() {
            Some(transaction) => transaction.commit().await,
            None => Ok(()),
        }
    }

    pub(crate) fn transaction_mut(
        &mut self,
    ) -> Result<&mut PgTransaction, CompetitionTransactionError> {
        self.transaction
            .as_mut()
            .ok_or_else(|| CompetitionTransactionError::Failed("竞价事务已经结束".to_owned()))
    }
}

// 公开事务端口使用 impl Future 以保持适配器不依赖宏；这里必须保留同一签名。
#[allow(clippy::manual_async_fn)]
impl TransactionRunner for PostgresStore {
    type Transaction = PostgresCompetitionTransaction;

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

            for attempt in 0..policy.max_attempts {
                let transaction = match self.pool().begin().await {
                    Ok(transaction) => transaction,
                    Err(error) => return Err(E::from(map_sql_error(error))),
                };
                let mut transaction = PostgresCompetitionTransaction::new(transaction);

                if let Err(error) =
                    set_isolation(transaction.transaction_mut(), policy.isolation).await
                {
                    let retryable =
                        matches!(error, CompetitionTransactionError::SerializationConflict);
                    if retryable && attempt + 1 < policy.max_attempts {
                        continue;
                    }
                    return Err(E::from(error));
                }

                match operation(&mut transaction).await {
                    Ok(value) => match transaction.commit().await {
                        Ok(()) => return Ok(value),
                        Err(error) => {
                            let error = map_sql_error(error);
                            if matches!(error, CompetitionTransactionError::SerializationConflict)
                                && attempt + 1 < policy.max_attempts
                            {
                                continue;
                            }
                            return Err(E::from(error));
                        }
                    },
                    Err(error) => {
                        if error.is_serialization_conflict() && attempt + 1 < policy.max_attempts {
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

// 公开事务端口使用 impl Future；具体适配器用 async block 绑定借用事务的生命周期。
#[allow(clippy::manual_async_fn)]
impl CompetitionTransaction for PostgresCompetitionTransaction {
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
impl AuctionLifecycleTransaction for PostgresCompetitionTransaction {
    fn load_lifecycle_context_for_update(
        &mut self,
        auction_id: AuctionId,
    ) -> impl Future<Output = Result<Option<AuctionLifecycleContext>, CompetitionTransactionError>> + Send
    {
        async move { load_lifecycle_context_postgres(self, auction_id).await }
    }

    fn apply_lifecycle_plan(
        &mut self,
        command: &AuctionLifecycleCommand,
        plan: AuctionLifecyclePlan,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        let command = command.clone();
        async move { apply_lifecycle_plan_postgres(self, &command, plan).await }
    }

    fn list_final_lot_ids_for_update(
        &mut self,
        auction_id: AuctionId,
    ) -> impl Future<Output = Result<Vec<LotId>, CompetitionTransactionError>> + Send {
        async move {
            let rows = {
                let transaction = self.transaction_mut()?;
                sqlx::query(
                    "SELECT id FROM public.lots
                     WHERE auction_id = $1 AND status = 'live'
                     ORDER BY lot_number
                     FOR UPDATE",
                )
                .bind(auction_id.as_uuid())
                .fetch_all(&mut **transaction)
                .await
                .map_err(map_sql_error)?
            };
            rows.into_iter()
                .map(|row| row.try_get("id").map(LotId::from_uuid).map_err(map_sql_error))
                .collect()
        }
    }

    fn proxy_user_is_eligible_for_update(
        &mut self,
        user_id: UserId,
    ) -> impl Future<Output = Result<bool, CompetitionTransactionError>> + Send {
        async move {
            let status = {
                let transaction = self.transaction_mut()?;
                sqlx::query_scalar::<_, String>(
                    "SELECT status FROM public.users WHERE id = $1 FOR UPDATE",
                )
                .bind(user_id.as_uuid())
                .fetch_optional(&mut **transaction)
                .await
                .map_err(map_sql_error)?
                .ok_or_else(|| invalid_state("活动代理用户不存在"))?
            };
            let blacklist_active = {
                let transaction = self.transaction_mut()?;
                sqlx::query_scalar::<_, bool>(
                    "SELECT revoked_at IS NULL
                     FROM public.user_blacklist
                     WHERE user_id = $1
                     FOR UPDATE",
                )
                .bind(user_id.as_uuid())
                .fetch_optional(&mut **transaction)
                .await
                .map_err(map_sql_error)?
                .unwrap_or(false)
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

async fn load_lifecycle_context_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    auction_id: AuctionId,
) -> Result<Option<AuctionLifecycleContext>, CompetitionTransactionError> {
    let row = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            r#"SELECT
                a.id, a.title, a.status, a.state_version, a.created_at, a.updated_at,
                a.scheduled_end_at, a.effective_end_at,
                a.cancelled_at, a.cancelled_reason, a.cancelled_by,
                a.world_id, a.sequence, a.preview_scheduled_at, a.scheduled_open_at,
                a.preview_manual_open,
                (a.policy_snapshot ->> 'scheduled_end_at')::timestamptz
                    AS policy_scheduled_end_at,
                (a.policy_snapshot ->> 'final_phase_lead_seconds')::bigint
                    AS policy_final_phase_lead_seconds,
                (SELECT round.local_week
                 FROM public.scheduled_rounds AS round
                 WHERE round.auction_id = a.id
                 LIMIT 1) AS scheduled_round_local_week
             FROM public.auctions AS a
             WHERE a.id = $1
             FOR UPDATE OF a"#,
        )
        .bind(auction_id.as_uuid())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    let Some(row) = row else {
        return Ok(None);
    };
    let state_version =
        positive_u64(row.try_get("state_version").map_err(map_sql_error)?, "拍卖状态版本无效")?;
    let auction = Auction {
        id: AuctionId::from_uuid(row.try_get("id").map_err(map_sql_error)?),
        title: row.try_get("title").map_err(map_sql_error)?,
        status: parse_auction_status(row.try_get::<String, _>("status").map_err(map_sql_error)?)?,
        state_version,
        created_at: row.try_get("created_at").map_err(map_sql_error)?,
        updated_at: row.try_get("updated_at").map_err(map_sql_error)?,
        scheduled_end_at: row.try_get("scheduled_end_at").map_err(map_sql_error)?,
        effective_end_at: row.try_get("effective_end_at").map_err(map_sql_error)?,
        cancelled_at: row.try_get("cancelled_at").map_err(map_sql_error)?,
        cancelled_reason: row.try_get("cancelled_reason").map_err(map_sql_error)?,
        cancelled_by: row.try_get("cancelled_by").map_err(map_sql_error)?,
    };
    let final_phase_lead_seconds: i64 =
        row.try_get("policy_final_phase_lead_seconds").map_err(map_sql_error)?;
    if final_phase_lead_seconds <= 0 {
        return Err(invalid_state("最终阶段领先时间无效"));
    }
    Ok(Some(AuctionLifecycleContext {
        auction,
        world_id: row.try_get("world_id").map_err(map_sql_error)?,
        sequence: row.try_get("sequence").map_err(map_sql_error)?,
        preview_scheduled_at: row.try_get("preview_scheduled_at").map_err(map_sql_error)?,
        scheduled_open_at: row.try_get("scheduled_open_at").map_err(map_sql_error)?,
        policy_scheduled_end_at: row.try_get("policy_scheduled_end_at").map_err(map_sql_error)?,
        policy_final_phase_lead: Duration::seconds(final_phase_lead_seconds),
        preview_manual_open: row.try_get("preview_manual_open").map_err(map_sql_error)?,
        scheduled_round_local_week: row
            .try_get::<Option<NaiveDate>, _>("scheduled_round_local_week")
            .map_err(map_sql_error)?,
    }))
}

async fn apply_lifecycle_plan_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    plan: AuctionLifecyclePlan,
) -> Result<(), CompetitionTransactionError> {
    if plan.auction.id != command.auction_id || plan.effect.action() != command.action {
        return Err(invalid_state("生命周期命令与动作计划不一致"));
    }
    let AuctionLifecyclePlan { auction, effect } = plan;
    match effect {
        AuctionLifecycleEffect::Preview { manual, successor } => {
            apply_preview_plan_postgres(transaction, command, &auction, manual, &successor).await
        }
        AuctionLifecycleEffect::Open { manual_override } => {
            apply_open_plan_postgres(transaction, command, &auction, manual_override).await
        }
        AuctionLifecycleEffect::Cancel => {
            apply_cancel_plan_postgres(transaction, command, &auction).await
        }
        AuctionLifecycleEffect::FinalPhase { proxy_plans } => {
            apply_final_plan_postgres(transaction, command, &auction, proxy_plans).await
        }
    }
}

async fn apply_preview_plan_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    manual: bool,
    successor: &AuctionPreviewSuccessorPlan,
) -> Result<(), CompetitionTransactionError> {
    let previous_version = previous_lifecycle_version(auction)?;
    let next_version = i64_version(auction.state_version, "预览状态版本")?;
    let scope = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "SELECT a.world_id, a.predecessor_auction_id, world.enabled
             FROM public.auctions AS a
             INNER JOIN public.worlds AS world ON world.id = a.world_id
             WHERE a.id = $1
             FOR UPDATE OF a, world",
        )
        .bind(auction.id.as_uuid())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    let Some(scope) = scope else {
        return Err(CompetitionTransactionError::NotFound);
    };
    let world_id: String = scope.try_get("world_id").map_err(map_sql_error)?;
    if !scope.try_get::<bool, _>("enabled").map_err(map_sql_error)? {
        return Err(CompetitionTransactionError::InvalidState);
    }
    if let Some(predecessor_id) =
        scope.try_get::<Option<Uuid>, _>("predecessor_auction_id").map_err(map_sql_error)?
    {
        let predecessor = {
            let transaction = transaction.transaction_mut()?;
            sqlx::query("SELECT world_id, status FROM public.auctions WHERE id = $1 FOR UPDATE")
                .bind(predecessor_id)
                .fetch_optional(&mut **transaction)
                .await
                .map_err(map_sql_error)?
        };
        let Some(predecessor) = predecessor else {
            return Err(invalid_state("预览拍卖的前驱记录不存在"));
        };
        let predecessor_world: String = predecessor.try_get("world_id").map_err(map_sql_error)?;
        let predecessor_status: String = predecessor.try_get("status").map_err(map_sql_error)?;
        if predecessor_world != world_id
            || !matches!(predecessor_status.as_str(), "settled" | "cancelled")
        {
            return Err(CompetitionTransactionError::InvalidState);
        }
    }

    let intake_target = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "SELECT auction_id, state_version
             FROM public.auction_intake_targets
             WHERE world_id = $1
             FOR UPDATE",
        )
        .bind(&world_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    let Some(intake_target) = intake_target else {
        return Err(CompetitionTransactionError::InvalidState);
    };
    let target_auction_id: Uuid = intake_target.try_get("auction_id").map_err(map_sql_error)?;
    let target_version: i64 = intake_target.try_get("state_version").map_err(map_sql_error)?;
    if target_auction_id != auction.id.as_uuid() || target_version <= 0 {
        return Err(CompetitionTransactionError::InvalidState);
    }

    let preview_revision = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query_scalar::<_, i64>(
            "UPDATE public.auctions
             SET status = 'preview',
                 preview_opened_at = COALESCE(preview_opened_at, $1),
                 preview_manual_open = preview_manual_open OR $2,
                 preview_revision = preview_revision + 1,
                 state_version = $3,
                 updated_at = $1
             WHERE id = $4 AND status = 'draft' AND state_version = $5
               AND ($2 OR preview_scheduled_at <= $1)
             RETURNING preview_revision",
        )
        .bind(auction.updated_at)
        .bind(manual)
        .bind(next_version)
        .bind(auction.id.as_uuid())
        .bind(previous_version)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    let Some(preview_revision) = preview_revision else {
        return Err(CompetitionTransactionError::InvalidState);
    };
    if preview_revision <= 0 {
        return Err(invalid_state("预览修订版本无效"));
    }

    let materialized = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "INSERT INTO public.auction_preview_items
                (auction_id, equipment_id, display_order, start_price, source_revision, collected_at)
             SELECT lot.auction_id, lot.equipment_id, lot.lot_number, lot.start_price,
                    auction.preview_revision, equipment.collected_at
             FROM public.lots AS lot
             INNER JOIN public.auctions AS auction ON auction.id = lot.auction_id
             INNER JOIN public.equipment AS equipment ON equipment.id = lot.equipment_id
             WHERE lot.auction_id = $1 AND lot.status = 'pending' AND auction.status = 'preview'
             ORDER BY lot.lot_number",
        )
        .bind(auction.id.as_uuid())
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?
        .rows_affected()
    };
    let pending_count: i64 = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query_scalar(
            "SELECT COUNT(*)::bigint FROM public.lots
             WHERE auction_id = $1 AND status = 'pending'",
        )
        .bind(auction.id.as_uuid())
        .fetch_one(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    if i64::try_from(materialized).ok() != Some(pending_count) {
        return Err(invalid_state("预览物化条目数与 pending lot 不一致"));
    }

    let successor_row = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            r#"INSERT INTO public.auctions
                (id, world_id, sequence, title, status, preview_scheduled_at,
                 scheduled_open_at, default_start_price, soft_close_window_seconds,
                 soft_close_extension_seconds, cancel_cutoff_seconds,
                 proxy_slow_interval_seconds, policy_snapshot, predecessor_auction_id,
                 created_at, updated_at)
             SELECT $1, current.world_id, $2, $3, 'draft', $4, $5,
                    current.default_start_price, current.soft_close_window_seconds,
                    current.soft_close_extension_seconds, current.cancel_cutoff_seconds,
                    current.proxy_slow_interval_seconds,
                    jsonb_set(current.policy_snapshot, '{scheduled_end_at}',
                              to_jsonb($6::timestamptz), true),
                    current.id, $7, $7
             FROM public.auctions AS current
             WHERE current.id = $8 AND current.status = 'preview'
             RETURNING world_id, state_version"#,
        )
        .bind(successor.auction_id.as_uuid())
        .bind(successor.sequence)
        .bind(&successor.title)
        .bind(successor.preview_due_at)
        .bind(successor.open_due_at)
        .bind(successor.scheduled_end_at)
        .bind(auction.updated_at)
        .bind(auction.id.as_uuid())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    let Some(successor_row) = successor_row else {
        return Err(CompetitionTransactionError::InvalidState);
    };
    let successor_world: String = successor_row.try_get("world_id").map_err(map_sql_error)?;
    let successor_version: i64 = successor_row.try_get("state_version").map_err(map_sql_error)?;
    if successor_world != world_id || successor_version != 1 {
        return Err(invalid_state("下一期草稿不满足初始状态不变量"));
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
                "state_version": successor_version,
            }))
            .map_err(|_| invalid_record())?,
        })
        .await?;

    {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "INSERT INTO public.scheduled_rounds
                (id, world_id, local_week, auction_id, status, preview_due_at,
                 open_due_at, scheduled_end_at, create_command_id, created_at, updated_at)
             VALUES ($1, $2, $3, $4, 'active', $5, $6, $7, $8, $9, $9)",
        )
        .bind(successor.round_id)
        .bind(&world_id)
        .bind(successor.local_week)
        .bind(successor.auction_id.as_uuid())
        .bind(successor.preview_due_at)
        .bind(successor.open_due_at)
        .bind(successor.scheduled_end_at)
        .bind(successor.create_command_id.as_uuid())
        .bind(auction.updated_at)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;

        let moved = sqlx::query(
            "UPDATE public.auction_intake_targets
             SET auction_id = $1, state_version = state_version + 1, updated_at = $2
             WHERE world_id = $3 AND auction_id = $4 AND state_version = $5",
        )
        .bind(successor.auction_id.as_uuid())
        .bind(auction.updated_at)
        .bind(&world_id)
        .bind(auction.id.as_uuid())
        .bind(target_version)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
        if moved.rows_affected() != 1 {
            return Err(CompetitionTransactionError::InvalidState);
        }

        if manual {
            sqlx::query(
                "UPDATE public.scheduled_rounds
                 SET preview_command_id = $1, preview_completed_at = $2,
                     state_version = state_version + 1, updated_at = $2
                 WHERE auction_id = $3 AND status = 'active'
                   AND preview_completed_at IS NULL",
            )
            .bind(command.command_id.as_uuid())
            .bind(auction.updated_at)
            .bind(auction.id.as_uuid())
            .execute(&mut **transaction)
            .await
            .map_err(map_sql_error)?;
        }
    }
    Ok(())
}

async fn apply_open_plan_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    manual_override: bool,
) -> Result<(), CompetitionTransactionError> {
    let scheduled_end_at =
        auction.scheduled_end_at.ok_or_else(|| invalid_state("开拍计划缺少名义结束时间"))?;
    if auction.effective_end_at != Some(scheduled_end_at) {
        return Err(invalid_state("开拍计划的名义与有效结束时间不一致"));
    }
    let lot_rows = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "SELECT id, status, submission_id
             FROM public.lots
             WHERE auction_id = $1
             ORDER BY lot_number
             FOR UPDATE",
        )
        .bind(auction.id.as_uuid())
        .fetch_all(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    if lot_rows.is_empty()
        || lot_rows.iter().any(|row| {
            row.try_get::<String, _>("status").ok().as_deref() != Some("pending")
                || row.try_get::<Option<Uuid>, _>("submission_id").ok().flatten().is_none()
        })
    {
        return Err(CompetitionTransactionError::InvalidState);
    }
    let activated = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "UPDATE public.lots
             SET status = 'live', current_price = start_price, effective_end_at = $1,
                 slow_due_at = NULL, slow_authorization_id = NULL, slow_generation = 0,
                 last_proxy_sequence = 0, state_version = state_version + 1, updated_at = $2
             WHERE auction_id = $3 AND status = 'pending'",
        )
        .bind(scheduled_end_at)
        .bind(auction.updated_at)
        .bind(auction.id.as_uuid())
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?
        .rows_affected()
    };
    if usize::try_from(activated).ok() != Some(lot_rows.len()) {
        return Err(CompetitionTransactionError::InvalidState);
    }

    let updated = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "UPDATE public.auctions
             SET status = 'live',
                 preview_opened_at = COALESCE(preview_opened_at, $1),
                 preview_scheduled_at = CASE
                     WHEN $2 AND preview_scheduled_at > $1 THEN $1 ELSE preview_scheduled_at END,
                 scheduled_open_at = CASE WHEN $2 THEN $1 ELSE scheduled_open_at END,
                 opened_at = $1, scheduled_end_at = $3, effective_end_at = $3,
                 state_version = $4, updated_at = $1
             WHERE id = $5 AND status = 'preview' AND state_version = $6
               AND (preview_scheduled_at <= $1 OR $2)
               AND (scheduled_open_at <= $1 OR $2)",
        )
        .bind(auction.updated_at)
        .bind(manual_override)
        .bind(scheduled_end_at)
        .bind(i64_version(auction.state_version, "开拍状态版本")?)
        .bind(auction.id.as_uuid())
        .bind(previous_lifecycle_version(auction)?)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    if updated.rows_affected() != 1 {
        return Err(CompetitionTransactionError::InvalidState);
    }

    if command.actor.actor_type == auction_application::AuctionLifecycleActorType::Admin {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "UPDATE public.scheduled_rounds
             SET open_command_id = $1, open_completed_at = $2,
                 state_version = state_version + 1, updated_at = $2
             WHERE auction_id = $3 AND status = 'active'
               AND preview_completed_at IS NOT NULL AND open_completed_at IS NULL",
        )
        .bind(command.command_id.as_uuid())
        .bind(auction.updated_at)
        .bind(auction.id.as_uuid())
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    }
    Ok(())
}

async fn apply_cancel_plan_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
) -> Result<(), CompetitionTransactionError> {
    let previous_status: String = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query_scalar("SELECT status FROM public.auctions WHERE id = $1 FOR UPDATE")
            .bind(auction.id.as_uuid())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(map_sql_error)?
            .ok_or(CompetitionTransactionError::NotFound)?
    };
    if !matches!(previous_status.as_str(), "draft" | "preview") {
        return Err(CompetitionTransactionError::InvalidState);
    }
    let lot_rows = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "SELECT l.id, l.status, l.state_version, l.equipment_id, l.submission_id,
                    s.seller,
                    COALESCE((a.policy_snapshot->'settlement'->>'version')::integer, 0)
                        AS policy_version,
                    a.world_id
             FROM public.lots AS l
             INNER JOIN public.auctions AS a ON a.id = l.auction_id
             LEFT JOIN public.auction_submissions AS s ON s.id = l.submission_id
             WHERE l.auction_id = $1
             ORDER BY l.lot_number
             FOR UPDATE OF l",
        )
        .bind(auction.id.as_uuid())
        .fetch_all(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    for lot in lot_rows {
        let status: String = lot.try_get("status").map_err(map_sql_error)?;
        if status == "withdrawn" {
            // 已由单 lot 撤回命令生成过返还义务，取消命令不能重复建立第二份责任。
            continue;
        }
        if status != "pending" {
            return Err(invalid_state("取消拍卖包含非 pending/withdrawn 标的"));
        }
        let lot_id: Uuid = lot.try_get("id").map_err(map_sql_error)?;
        let equipment_id: Uuid = lot.try_get("equipment_id").map_err(map_sql_error)?;
        let submission_id: Option<Uuid> = lot.try_get("submission_id").map_err(map_sql_error)?;
        let seller: Option<String> = lot.try_get("seller").map_err(map_sql_error)?;
        let seller = seller
            .filter(|value| !value.trim().is_empty() && value == value.trim())
            .ok_or_else(|| invalid_state("取消返还义务缺少卖方游戏身份"))?;
        let world_id: String = lot.try_get("world_id").map_err(map_sql_error)?;
        if !matches!(world_id.as_str(), "main" | "isekai") {
            return Err(invalid_state("取消返还义务的世界不在允许集合内"));
        }
        let policy_version: i32 = lot.try_get("policy_version").map_err(map_sql_error)?;
        if policy_version < 0 {
            return Err(invalid_state("取消返还义务的策略版本无效"));
        }
        let lot_state_version: i64 = lot.try_get("state_version").map_err(map_sql_error)?;
        if lot_state_version <= 0 {
            return Err(invalid_state("取消标的状态版本无效"));
        }

        let withdrawn = {
            let transaction = transaction.transaction_mut()?;
            sqlx::query(
                "UPDATE public.lots
                 SET status = 'withdrawn', slow_authorization_id = NULL, slow_due_at = NULL,
                     state_version = state_version + 1, updated_at = $1
                 WHERE id = $2 AND auction_id = $3 AND status = 'pending' AND state_version = $4",
            )
            .bind(auction.updated_at)
            .bind(lot_id)
            .bind(auction.id.as_uuid())
            .bind(lot_state_version)
            .execute(&mut **transaction)
            .await
            .map_err(map_sql_error)?
        };
        if withdrawn.rows_affected() != 1 {
            return Err(CompetitionTransactionError::StaleState);
        }
        insert_cancellation_return_obligation_postgres(
            transaction,
            command,
            auction,
            lot_id,
            equipment_id,
            submission_id,
            &world_id,
            &seller,
            policy_version,
        )
        .await?;
    }

    {
        let transaction = transaction.transaction_mut()?;
        if previous_status == "draft" {
            sqlx::query(
                "DELETE FROM public.auction_intake_targets
                 WHERE auction_id = $1",
            )
            .bind(auction.id.as_uuid())
            .execute(&mut **transaction)
            .await
            .map_err(map_sql_error)?;
        }
        sqlx::query(
            "UPDATE public.scheduled_rounds
             SET status = 'missed', auction_id = NULL,
                 lease_owner = NULL, lease_until = NULL,
                 state_version = state_version + 1, updated_at = $1
             WHERE auction_id = $2 AND status = 'active' AND open_completed_at IS NULL",
        )
        .bind(auction.updated_at)
        .bind(auction.id.as_uuid())
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
        let updated = sqlx::query(
            "UPDATE public.auctions
             SET status = 'cancelled', state_version = $1, updated_at = $2,
                 cancelled_at = $3, cancelled_reason = $4, cancelled_by = $5
             WHERE id = $6 AND status = $7 AND state_version = $8",
        )
        .bind(i64_version(auction.state_version, "取消状态版本")?)
        .bind(auction.updated_at)
        .bind(auction.cancelled_at)
        .bind(&auction.cancelled_reason)
        .bind(&auction.cancelled_by)
        .bind(auction.id.as_uuid())
        .bind(&previous_status)
        .bind(previous_lifecycle_version(auction)?)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
        if updated.rows_affected() != 1 {
            return Err(CompetitionTransactionError::InvalidState);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_cancellation_return_obligation_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    lot_id: Uuid,
    equipment_id: Uuid,
    submission_id: Option<Uuid>,
    world_id: &str,
    seller: &str,
    policy_version: i32,
) -> Result<(), CompetitionTransactionError> {
    let obligation_id = deterministic_uuid(
        command.command_id,
        &format!("settlement-obligation:{lot_id}:withdrawn_return"),
        1,
    );
    let transaction = transaction.transaction_mut()?;
    sqlx::query(
        "INSERT INTO public.settlement_obligations
            (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
             obligation_kind, business_state, state_version, recipient_user_id,
             recipient_game_identity, amount, carrier_credit, equipment_id, submission_id,
             prerequisite_obligation_id, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, 'withdrawn_return', 'ready', 1, NULL,
                 NULLIF($7, ''), 0, 0, $8, $9, NULL, $10, $10)",
    )
    .bind(obligation_id)
    .bind(auction.id.as_uuid())
    .bind(lot_id)
    .bind(world_id)
    .bind(command.command_id.as_uuid())
    .bind(policy_version)
    .bind(seller)
    .bind(equipment_id)
    .bind(submission_id)
    .bind(auction.updated_at)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        "INSERT INTO public.settlement_obligation_fulfillments
            (obligation_id, execution_state, dispatch_world, delivery_operation)
         VALUES ($1, 'pending', $2, 'return')",
    )
    .bind(obligation_id)
    .bind(world_id)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

async fn apply_final_plan_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    proxy_plans: Vec<auction_application::AuctionFinalProxyPlan>,
) -> Result<(), CompetitionTransactionError> {
    let updated = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "UPDATE public.auctions
             SET status = 'final', final_phase_at = $1, state_version = $2, updated_at = $1
             WHERE id = $3 AND status = 'live' AND state_version = $4",
        )
        .bind(auction.updated_at)
        .bind(i64_version(auction.state_version, "最终阶段状态版本")?)
        .bind(auction.id.as_uuid())
        .bind(previous_lifecycle_version(auction)?)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    if updated.rows_affected() != 1 {
        return Err(CompetitionTransactionError::InvalidState);
    }
    for proxy_plan in proxy_plans {
        if proxy_plan.command.aggregate_id != proxy_plan.competition.final_lot.lot_id.as_uuid() {
            return Err(invalid_state("最终阶段代理子命令与 lot 计划不一致"));
        }
        transaction.insert_command_inner(proxy_plan.command).await?;
        transaction.apply_plan_inner(proxy_plan.competition, command.request_id.clone()).await?;
    }
    if command.actor.actor_type == auction_application::AuctionLifecycleActorType::Admin {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "UPDATE public.scheduled_rounds
             SET final_command_id = $1, final_completed_at = $2,
                 lease_owner = NULL, lease_until = NULL,
                 state_version = state_version + 1, updated_at = $2
             WHERE auction_id = $3 AND status = 'active'
               AND open_completed_at IS NOT NULL AND final_completed_at IS NULL",
        )
        .bind(command.command_id.as_uuid())
        .bind(auction.updated_at)
        .bind(auction.id.as_uuid())
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    }
    Ok(())
}

fn previous_lifecycle_version(auction: &Auction) -> Result<i64, CompetitionTransactionError> {
    auction
        .state_version
        .checked_sub(1)
        .and_then(|version| i64::try_from(version).ok())
        .ok_or_else(|| invalid_state("拍卖状态版本无法回退校验"))
}

#[allow(clippy::manual_async_fn)]
impl AdminAuctionCreateTransaction for PostgresCompetitionTransaction {
    fn create_admin_auction(
        &mut self,
        command: AdminCreateAuctionCommand,
        title: String,
        policy: AuctionPolicyTemplate,
        created_at: DateTime<Utc>,
    ) -> impl Future<Output = Result<AdminAuctionSummary, CompetitionTransactionError>> + Send {
        async move { create_admin_auction_inner(self, &command, &title, &policy, created_at).await }
    }

    fn append_admin_auction_created_record(
        &mut self,
        command: AdminCreateAuctionCommand,
        result: AdminAuctionSummary,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        async move { append_admin_auction_created_record_inner(self, &command, &result).await }
    }
}

#[allow(clippy::manual_async_fn)]
impl AdminLotUpdateTransaction for PostgresCompetitionTransaction {
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

    fn load_admin_lot_for_update(
        &mut self,
        lot_id: LotId,
    ) -> impl Future<Output = Result<Option<AdminLotUpdateSnapshot>, CompetitionTransactionError>> + Send
    {
        async move { load_admin_lot_update_inner(self, lot_id).await }
    }

    fn update_admin_lot(
        &mut self,
        command: &AdminLotUpdateCommand,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<AdminLotSummary, CompetitionTransactionError>> + Send {
        let command = command.clone();
        async move { update_admin_lot_inner(self, &command, now).await }
    }

    fn advance_admin_preview_revision(
        &mut self,
        auction_id: AuctionId,
        expected_revision: i64,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<i64, CompetitionTransactionError>> + Send {
        async move {
            let next_revision = expected_revision
                .checked_add(1)
                .ok_or_else(|| invalid_state("Admin lot 预览修订溢出"))?;
            let row = {
                let transaction = self.transaction_mut()?;
                sqlx::query(
                    "UPDATE public.auctions
                     SET preview_revision = $1,
                         state_version = state_version + 1,
                         updated_at = $2
                     WHERE id = $3 AND status = 'draft' AND preview_revision = $4
                     RETURNING state_version",
                )
                .bind(next_revision)
                .bind(now)
                .bind(auction_id.as_uuid())
                .bind(expected_revision)
                .fetch_optional(&mut **transaction)
                .await
                .map_err(map_sql_error)?
            };
            let Some(row) = row else {
                return Err(CompetitionTransactionError::StaleState);
            };
            row.try_get("state_version").map_err(map_sql_error)
        }
    }

    fn insert_withdrawn_return_obligation(
        &mut self,
        command: &AdminLotUpdateCommand,
        snapshot: &AdminLotUpdateSnapshot,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        let command = command.clone();
        let snapshot = snapshot.clone();
        async move { insert_withdrawn_return_obligation_inner(self, &command, &snapshot, now).await }
    }

    fn append_admin_lot_record(
        &mut self,
        command: &AdminLotUpdateCommand,
        result: &AdminLotSummary,
        changed: bool,
        auction_state_version: i64,
        preview_revision: i64,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        let command = command.clone();
        let result = result.clone();
        async move {
            append_admin_lot_record_inner(
                self,
                &command,
                &result,
                changed,
                auction_state_version,
                preview_revision,
            )
            .await
        }
    }
}

async fn load_admin_lot_update_inner(
    transaction: &mut PostgresCompetitionTransaction,
    lot_id: LotId,
) -> Result<Option<AdminLotUpdateSnapshot>, CompetitionTransactionError> {
    let scope = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "SELECT a.id AS auction_id, a.world_id
             FROM public.lots AS l
             INNER JOIN public.auctions AS a ON a.id = l.auction_id
             WHERE l.id = $1",
        )
        .bind(lot_id.as_uuid())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    let Some(scope) = scope else {
        return Ok(None);
    };
    let auction_id: Uuid = scope.try_get("auction_id").map_err(map_sql_error)?;
    let world_id: String = scope.try_get("world_id").map_err(map_sql_error)?;

    {
        let transaction = transaction.transaction_mut()?;
        let world_exists: Option<String> =
            sqlx::query_scalar("SELECT id FROM public.worlds WHERE id = $1 FOR UPDATE")
                .bind(&world_id)
                .fetch_optional(&mut **transaction)
                .await
                .map_err(map_sql_error)?;
        if world_exists.is_none() {
            return Ok(None);
        }
        let auction_exists: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM public.auctions WHERE id = $1 FOR UPDATE")
                .bind(auction_id)
                .fetch_optional(&mut **transaction)
                .await
                .map_err(map_sql_error)?;
        if auction_exists.is_none() {
            return Ok(None);
        }
    }

    let row = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "SELECT
                a.status AS auction_status, a.state_version AS auction_state_version,
                a.preview_revision,
                COALESCE((a.policy_snapshot->'settlement'->>'version')::integer, 0)
                    AS policy_version,
                l.id, l.auction_id, l.equipment_id, l.submission_id, l.lot_number, l.status,
                l.start_price, COALESCE(l.current_price, l.start_price) AS current_price,
                l.leader_user_id, l.effective_end_at, l.slow_due_at, l.slow_generation,
                l.slow_authorization_id, l.state_version, l.created_at, l.updated_at,
                e.external_equipment_id, e.name AS equipment_name, e.category AS equipment_category,
                e.attributes AS equipment_attributes, e.collected_at AS equipment_collected_at,
                s.seller, s.source_kind AS submission_source_kind,
                s.source_reference AS submission_source_reference, s.mailbox_key,
                s.external_mail_id, s.received_at AS submission_received_at
             FROM public.lots AS l
             INNER JOIN public.auctions AS a ON a.id = l.auction_id
             INNER JOIN public.equipment AS e ON e.id = l.equipment_id
             LEFT JOIN public.auction_submissions AS s ON s.id = l.submission_id
             WHERE l.id = $1 AND a.id = $2
             FOR UPDATE OF l",
        )
        .bind(lot_id.as_uuid())
        .bind(auction_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    let Some(row) = row else {
        return Ok(None);
    };
    let auction_status: String = row.try_get("auction_status").map_err(map_sql_error)?;
    let auction_state_version: i64 = row.try_get("auction_state_version").map_err(map_sql_error)?;
    let preview_revision: i64 = row.try_get("preview_revision").map_err(map_sql_error)?;
    let policy_version: i32 = row.try_get("policy_version").map_err(map_sql_error)?;
    let lot = map_admin_lot_row(row)?;
    let snapshot = AdminLotUpdateSnapshot {
        lot,
        auction_status,
        world_id,
        auction_state_version,
        preview_revision,
        policy_version,
    };
    if !snapshot.validate() {
        return Err(invalid_state("Admin lot 锁内投影校验失败"));
    }
    Ok(Some(snapshot))
}

async fn update_admin_lot_inner(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AdminLotUpdateCommand,
    now: DateTime<Utc>,
) -> Result<AdminLotSummary, CompetitionTransactionError> {
    let start_price = command.start_price;
    let updated = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "UPDATE public.lots
             SET status = CASE WHEN $1 = 'withdraw' THEN 'withdrawn' ELSE 'pending' END,
                 start_price = CASE WHEN $1 = 'set_start_price' THEN $2 ELSE start_price END,
                 current_price = CASE WHEN $1 = 'set_start_price' THEN $2 ELSE current_price END,
                 state_version = state_version + 1,
                 updated_at = $3
             WHERE id = $4 AND auction_id = (
                 SELECT auction_id FROM public.lots WHERE id = $4
             ) AND status = 'pending' AND state_version = $5
             RETURNING id",
        )
        .bind(command.action.as_str())
        .bind(start_price)
        .bind(now)
        .bind(command.lot_id.as_uuid())
        .bind(command.expected_state_version)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    if updated.is_none() {
        return Err(CompetitionTransactionError::StaleState);
    }
    load_admin_lot_update_inner(transaction, command.lot_id)
        .await?
        .map(|snapshot| snapshot.lot)
        .ok_or(CompetitionTransactionError::NotFound)
}

async fn insert_withdrawn_return_obligation_inner(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AdminLotUpdateCommand,
    snapshot: &AdminLotUpdateSnapshot,
    now: DateTime<Utc>,
) -> Result<(), CompetitionTransactionError> {
    let seller = snapshot.lot.seller.as_deref().ok_or(CompetitionTransactionError::InvalidState)?;
    if !matches!(snapshot.world_id.as_str(), "main" | "isekai") {
        return Err(invalid_state("withdrawn-return 世界不在允许集合内"));
    }
    let obligation_id = deterministic_uuid(
        command.command_id,
        &format!("settlement-obligation:{}:withdrawn_return", snapshot.lot.id),
        1,
    );
    {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "INSERT INTO public.settlement_obligations
                (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
                 obligation_kind, business_state, state_version, recipient_user_id,
                 recipient_game_identity, amount, carrier_credit, equipment_id, submission_id,
                 prerequisite_obligation_id, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, 'withdrawn_return', 'ready', 1, NULL,
                     NULLIF($7, ''), 0, 0, $8, $9, NULL, $10, $10)",
        )
        .bind(obligation_id)
        .bind(snapshot.lot.auction_id.as_uuid())
        .bind(snapshot.lot.id.as_uuid())
        .bind(&snapshot.world_id)
        .bind(command.command_id.as_uuid())
        .bind(snapshot.policy_version)
        .bind(seller)
        .bind(snapshot.lot.equipment_id.as_uuid())
        .bind(snapshot.lot.submission_id.map(|id| id.as_uuid()))
        .bind(now)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
        sqlx::query(
            "INSERT INTO public.settlement_obligation_fulfillments
                (obligation_id, execution_state, dispatch_world, delivery_operation)
             VALUES ($1, 'pending', $2, 'return')",
        )
        .bind(obligation_id)
        .bind(&snapshot.world_id)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    }
    Ok(())
}

async fn append_admin_lot_record_inner(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AdminLotUpdateCommand,
    result: &AdminLotSummary,
    changed: bool,
    auction_state_version: i64,
    preview_revision: i64,
) -> Result<(), CompetitionTransactionError> {
    let metadata = json!({
        "command_id": command.command_id.as_uuid(),
        "action": command.action.as_str(),
        "lot_id": result.id.as_uuid(),
        "state_version": result.state_version,
        "changed": changed,
        "preview_revision": preview_revision,
    });
    let transaction = transaction.transaction_mut()?;
    sqlx::query(
        "INSERT INTO public.audit_records
            (id, actor_type, actor_id, action, entity_type, entity_id, result, request_id, source, metadata)
         VALUES ($1, 'admin', $2, $3, 'lot', $4, 'success', $5, 'admin', $6)",
    )
    .bind(Uuid::new_v4())
    .bind(&command.actor_id)
    .bind(format!("lot.{}", command.action.as_str()))
    .bind(result.id.as_uuid().to_string())
    .bind(&command.request_id)
    .bind(metadata)
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;

    if !changed {
        return Ok(());
    }

    let event_data = sqlx::query(
        "SELECT a.world_id, a.preview_manual_open,
                (SELECT COUNT(*)::bigint FROM public.auction_preview_items AS item
                 WHERE item.auction_id = a.id) AS item_count
         FROM public.auctions AS a WHERE a.id = $1",
    )
    .bind(result.auction_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let item_count: i64 = event_data.try_get("item_count").map_err(map_sql_error)?;
    let manual_open: bool = event_data.try_get("preview_manual_open").map_err(map_sql_error)?;
    let stream_key = format!("auction:{}", result.auction_id.as_uuid());
    sqlx::query(
        "INSERT INTO public.realtime_streams (stream_key)
         VALUES ($1) ON CONFLICT (stream_key) DO NOTHING",
    )
    .bind(&stream_key)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let sequence: i64 = sqlx::query_scalar(
        "SELECT next_sequence FROM public.realtime_streams WHERE stream_key = $1 FOR UPDATE",
    )
    .bind(&stream_key)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        "UPDATE public.realtime_streams
         SET next_sequence = next_sequence + 1, updated_at = CURRENT_TIMESTAMP
         WHERE stream_key = $1",
    )
    .bind(&stream_key)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        "INSERT INTO public.realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
             state_version, origin_command_id)
         VALUES ($1, $2, $3, 'auction.preview_updated', 1, $4, $5, $6)",
    )
    .bind(Uuid::new_v4())
    .bind(&stream_key)
    .bind(sequence)
    .bind(json!({
        "auction_id": result.auction_id.as_uuid(),
        "preview_revision": preview_revision,
        "item_count": item_count,
        "manual_open": manual_open,
        "state_version": auction_state_version,
    }))
    .bind(auction_state_version)
    .bind(command.command_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

fn map_admin_lot_row(row: PgRow) -> Result<AdminLotSummary, CompetitionTransactionError> {
    let attributes: sqlx::types::Json<Value> =
        row.try_get("equipment_attributes").map_err(map_sql_error)?;
    Ok(AdminLotSummary {
        id: LotId::from_uuid(row.try_get("id").map_err(map_sql_error)?),
        auction_id: AuctionId::from_uuid(row.try_get("auction_id").map_err(map_sql_error)?),
        equipment_id: auction_domain::EquipmentId::from_uuid(
            row.try_get("equipment_id").map_err(map_sql_error)?,
        ),
        submission_id: row
            .try_get::<Option<Uuid>, _>("submission_id")
            .map_err(map_sql_error)?
            .map(auction_domain::SubmissionId::from_uuid),
        lot_number: row.try_get("lot_number").map_err(map_sql_error)?,
        status: row.try_get("status").map_err(map_sql_error)?,
        start_price: row.try_get("start_price").map_err(map_sql_error)?,
        current_price: row.try_get("current_price").map_err(map_sql_error)?,
        leader_user_id: row
            .try_get::<Option<Uuid>, _>("leader_user_id")
            .map_err(map_sql_error)?
            .map(UserId::from_uuid),
        effective_end_at: row.try_get("effective_end_at").map_err(map_sql_error)?,
        slow_due_at: row.try_get("slow_due_at").map_err(map_sql_error)?,
        slow_generation: row.try_get("slow_generation").map_err(map_sql_error)?,
        slow_authorization_id: row
            .try_get::<Option<Uuid>, _>("slow_authorization_id")
            .map_err(map_sql_error)?
            .map(ProxyAuthorizationId::from_uuid),
        state_version: row.try_get("state_version").map_err(map_sql_error)?,
        created_at: row.try_get("created_at").map_err(map_sql_error)?,
        updated_at: row.try_get("updated_at").map_err(map_sql_error)?,
        external_equipment_id: row.try_get("external_equipment_id").map_err(map_sql_error)?,
        equipment_name: row.try_get("equipment_name").map_err(map_sql_error)?,
        equipment_category: row.try_get("equipment_category").map_err(map_sql_error)?,
        equipment_attributes: attributes.0,
        equipment_collected_at: row.try_get("equipment_collected_at").map_err(map_sql_error)?,
        seller: row.try_get("seller").map_err(map_sql_error)?,
        submission_source_kind: row.try_get("submission_source_kind").map_err(map_sql_error)?,
        submission_source_reference: row
            .try_get("submission_source_reference")
            .map_err(map_sql_error)?,
        mailbox_key: row.try_get("mailbox_key").map_err(map_sql_error)?,
        external_mail_id: row.try_get("external_mail_id").map_err(map_sql_error)?,
        submission_received_at: row.try_get("submission_received_at").map_err(map_sql_error)?,
    })
}

async fn create_admin_auction_inner(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AdminCreateAuctionCommand,
    title: &str,
    policy: &AuctionPolicyTemplate,
    created_at: DateTime<Utc>,
) -> Result<AdminAuctionSummary, CompetitionTransactionError> {
    let rules = policy
        .auction_rules()
        .map_err(|error| invalid_state(&format!("拍卖策略规则无效：{error}")))?;
    let policy_snapshot = policy.snapshot(command.scheduled_end_at);
    let world_enabled = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query_scalar::<_, bool>("SELECT enabled FROM public.worlds WHERE id = $1 FOR UPDATE")
            .bind(&command.world_id)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(map_sql_error)?
    };
    let Some(world_enabled) = world_enabled else {
        return Err(CompetitionTransactionError::NotFound);
    };
    if !world_enabled {
        return Err(CompetitionTransactionError::Conflict);
    }

    let existing_target = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query_scalar::<_, Uuid>(
            "SELECT auction_id FROM public.auction_intake_targets WHERE world_id = $1 FOR UPDATE",
        )
        .bind(&command.world_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    if existing_target.is_some() {
        return Err(CompetitionTransactionError::Conflict);
    }

    let row = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            r#"INSERT INTO public.auctions
                (id, world_id, sequence, title, status, preview_scheduled_at,
                 scheduled_open_at, default_start_price, soft_close_window_seconds,
                 soft_close_extension_seconds, cancel_cutoff_seconds,
                 proxy_slow_interval_seconds, policy_snapshot, created_at, updated_at)
             VALUES ($1, $2, $3, $4, 'draft', $5, $6, $7, $8, $9, $10, $11, $12, $13, $13)
             RETURNING state_version, created_at, updated_at"#,
        )
        .bind(command.auction_id.as_uuid())
        .bind(&command.world_id)
        .bind(command.sequence)
        .bind(title)
        .bind(command.preview_scheduled_at)
        .bind(command.scheduled_open_at)
        .bind(rules.default_start_price.value())
        .bind(rules.soft_close_window.num_seconds())
        .bind(rules.soft_close_extension.num_seconds())
        .bind(rules.cancel_cutoff.num_seconds())
        .bind(rules.proxy_slow_interval.num_seconds())
        .bind(policy_snapshot)
        .bind(created_at)
        .fetch_one(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    let state_version: i64 = row.try_get("state_version").map_err(map_sql_error)?;
    if state_version <= 0 {
        return Err(invalid_state("后台拍卖创建返回的状态版本无效"));
    }
    let created_at: DateTime<Utc> = row.try_get("created_at").map_err(map_sql_error)?;
    let updated_at: DateTime<Utc> = row.try_get("updated_at").map_err(map_sql_error)?;

    {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "INSERT INTO public.auction_intake_targets
                (world_id, auction_id, target_status, state_version, updated_at)
             VALUES ($1, $2, 'draft', $3, $4)",
        )
        .bind(&command.world_id)
        .bind(command.auction_id.as_uuid())
        .bind(state_version)
        .bind(updated_at)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
        let local_week = command.preview_scheduled_at.date_naive()
            - Duration::days(i64::from(
                command.preview_scheduled_at.weekday().num_days_from_monday(),
            ));
        let round_id = deterministic_uuid(command.command_id, "scheduled-round", 1);
        sqlx::query(
            "INSERT INTO public.scheduled_rounds
                (id, world_id, local_week, auction_id, status, preview_due_at,
                 open_due_at, scheduled_end_at, create_command_id, created_at, updated_at)
             VALUES ($1, $2, $3, $4, 'active', $5, $6, $7, $8, $9, $9)",
        )
        .bind(round_id)
        .bind(&command.world_id)
        .bind(local_week)
        .bind(command.auction_id.as_uuid())
        .bind(command.preview_scheduled_at)
        .bind(command.scheduled_open_at)
        .bind(command.scheduled_end_at)
        .bind(command.command_id.as_uuid())
        .bind(created_at)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    }

    // 旧 A03 创建接口把计划结束时间冻结在 policy_snapshot，拍卖生命周期列在真正开放前保持空值。
    let result = AdminAuctionSummary {
        auction: SettlementAuctionSnapshot {
            id: command.auction_id,
            world_id: command.world_id.clone(),
            sequence: command.sequence,
            status: "draft".to_owned(),
            preview_scheduled_at: command.preview_scheduled_at,
            scheduled_open_at: command.scheduled_open_at,
            preview_opened_at: None,
            preview_manual_open: false,
            preview_revision: 0,
            opened_at: None,
            final_phase_at: None,
            settled_at: None,
            cancelled_at: None,
            cancelled_reason: None,
            cancelled_by: None,
            state_version,
            scheduled_end_at: None,
            effective_end_at: None,
            created_at,
            updated_at,
        },
        scheduled_ends_at: None,
        lot_count: 0,
    };
    if !result.validate() {
        return Err(invalid_state("后台拍卖创建结果不满足生命周期不变量"));
    }
    Ok(result)
}

async fn append_admin_auction_created_record_inner(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AdminCreateAuctionCommand,
    result: &AdminAuctionSummary,
) -> Result<(), CompetitionTransactionError> {
    let metadata = json!({
        "command_id": command.command_id.as_uuid(),
        "auction_id": result.auction.id.as_uuid(),
        "world_id": result.auction.world_id,
        "sequence": result.auction.sequence,
        "state_version": result.auction.state_version,
    });
    let payload = json!({
        "auction_id": result.auction.id.as_uuid(),
        "world_id": result.auction.world_id,
        "sequence": result.auction.sequence,
        "state_version": result.auction.state_version,
    });
    let stream_key = format!("auction:{}", result.auction.id.as_uuid());
    let transaction = transaction.transaction_mut()?;
    sqlx::query(
        r#"INSERT INTO public.audit_records
            (id, actor_type, actor_id, action, entity_type, entity_id, result, request_id, source, metadata)
         VALUES ($1, 'admin', $2, 'auction.create', 'auction', $3, 'success', $4, 'admin', $5)"#,
    )
    .bind(Uuid::new_v4())
    .bind(&command.actor_id)
    .bind(result.auction.id.as_uuid().to_string())
    .bind(command.request_id.as_deref())
    .bind(metadata)
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;

    sqlx::query(
        "INSERT INTO public.realtime_streams (stream_key)
         VALUES ($1) ON CONFLICT (stream_key) DO NOTHING",
    )
    .bind(&stream_key)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let sequence: i64 = sqlx::query_scalar(
        "SELECT next_sequence FROM public.realtime_streams WHERE stream_key = $1 FOR UPDATE",
    )
    .bind(&stream_key)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        "UPDATE public.realtime_streams
         SET next_sequence = next_sequence + 1, updated_at = CURRENT_TIMESTAMP
         WHERE stream_key = $1",
    )
    .bind(&stream_key)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        r#"INSERT INTO public.realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
             state_version, origin_command_id)
         VALUES ($1, $2, $3, 'auction.created', 1, $4, $5, $6)"#,
    )
    .bind(Uuid::new_v4())
    .bind(&stream_key)
    .bind(sequence)
    .bind(&payload)
    .bind(result.auction.state_version)
    .bind(command.command_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;

    // 旧版 relay 会把创建事件同时投递到公开发现范围。这里在同一事务内追加一个持久化
    // 派生事件，使跨进程重启、HTTP catch-up 和 WebSocket 初始读取都能从权威数据库恢复。
    // 派生事件使用独立流序号和事件 ID，但保留同一命令、状态版本和公开负载；它不是第二份
    // 拍卖事实，也不能被当作拍卖专属流中的额外生命周期事件。
    let discovery_stream_key = "auction:discovery";
    sqlx::query(
        "INSERT INTO public.realtime_streams (stream_key)
         VALUES ($1) ON CONFLICT (stream_key) DO NOTHING",
    )
    .bind(discovery_stream_key)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let discovery_sequence: i64 = sqlx::query_scalar(
        "SELECT next_sequence FROM public.realtime_streams WHERE stream_key = $1 FOR UPDATE",
    )
    .bind(discovery_stream_key)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        "UPDATE public.realtime_streams
         SET next_sequence = next_sequence + 1, updated_at = CURRENT_TIMESTAMP
         WHERE stream_key = $1",
    )
    .bind(discovery_stream_key)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        r#"INSERT INTO public.realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
             state_version, origin_command_id)
         VALUES ($1, $2, $3, 'auction.created', 1, $4, $5, $6)"#,
    )
    .bind(Uuid::new_v4())
    .bind(discovery_stream_key)
    .bind(discovery_sequence)
    .bind(&payload)
    .bind(result.auction.state_version)
    .bind(command.command_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

async fn append_lifecycle_record_inner(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    result: &AuctionLifecycleResult,
) -> Result<(), CompetitionTransactionError> {
    let metadata = json!({
        "command_id": command.command_id.as_uuid(),
        "action": command.action.command_kind(),
        "status": auction_status_name(result.status),
        "state_version": result.state_version,
    });
    let transaction = transaction.transaction_mut()?;
    sqlx::query(
        r#"INSERT INTO public.audit_records
            (id, actor_type, actor_id, action, entity_type, entity_id, result, request_id, source, metadata)
         VALUES ($1, $2, $3, $4, 'auction', $5, 'success', $6, $7, $8)"#,
    )
    .bind(Uuid::new_v4())
    .bind(command.actor.actor_type.database_name())
    .bind(&command.actor.actor_id)
    .bind(command.action.command_kind())
    .bind(result.auction_id.as_uuid().to_string())
    .bind(command.request_id.as_deref())
    .bind(command.actor.actor_type.database_name())
    .bind(metadata)
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;

    let (event_kind, payload) = lifecycle_event_payload(transaction, command, result).await?;
    let stream_key = format!("auction:{}", result.auction_id.as_uuid());
    sqlx::query(
        "INSERT INTO public.realtime_streams (stream_key)
         VALUES ($1) ON CONFLICT (stream_key) DO NOTHING",
    )
    .bind(&stream_key)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let sequence: i64 = sqlx::query_scalar(
        "SELECT next_sequence FROM public.realtime_streams WHERE stream_key = $1 FOR UPDATE",
    )
    .bind(&stream_key)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        "UPDATE public.realtime_streams
         SET next_sequence = next_sequence + 1, updated_at = CURRENT_TIMESTAMP
         WHERE stream_key = $1",
    )
    .bind(&stream_key)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        r#"INSERT INTO public.realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
             state_version, origin_command_id)
         VALUES ($1, $2, $3, $4, 1, $5, $6, $7)"#,
    )
    .bind(Uuid::new_v4())
    .bind(&stream_key)
    .bind(sequence)
    .bind(event_kind)
    .bind(payload)
    .bind(i64_version(result.state_version, "生命周期事件状态版本")?)
    .bind(command.command_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

async fn lifecycle_event_payload(
    transaction: &mut PgTransaction,
    command: &AuctionLifecycleCommand,
    result: &AuctionLifecycleResult,
) -> Result<(&'static str, Value), CompetitionTransactionError> {
    let row = sqlx::query(
        r#"SELECT a.world_id,
                  a.preview_revision,
                  a.preview_manual_open,
                  (SELECT COUNT(*)::bigint
                   FROM public.auction_preview_items AS item
                   WHERE item.auction_id = a.id) AS item_count
           FROM public.auctions AS a
           WHERE a.id = $1"#,
    )
    .bind(result.auction_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let world_id: String = row.try_get("world_id").map_err(map_sql_error)?;
    let preview_revision: i64 = row.try_get("preview_revision").map_err(map_sql_error)?;
    let manual_open: bool = row.try_get("preview_manual_open").map_err(map_sql_error)?;
    let item_count: i64 = row.try_get("item_count").map_err(map_sql_error)?;
    let state_version = result.state_version;
    let event = match command.action {
        auction_application::AuctionLifecycleAction::Preview => (
            "auction.preview_updated",
            json!({
                "auction_id": result.auction_id.as_uuid(),
                "preview_revision": preview_revision,
                "item_count": item_count,
                "manual_open": manual_open,
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

impl PostgresCompetitionTransaction {
    pub(crate) async fn find_command_inner(
        &mut self,
        command_id: auction_domain::CommandId,
    ) -> Result<Option<StoredCommand>, CompetitionTransactionError> {
        let row = {
            let transaction = self.transaction_mut()?;
            sqlx::query(
                r#"SELECT command_kind, aggregate_id, request_hash, result_code, result_payload
                   FROM public.auction_commands
                   WHERE command_id = $1
                   FOR UPDATE"#,
            )
            .bind(command_id.as_uuid())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(map_sql_error)?
        };

        let Some(row) = row else {
            return Ok(None);
        };
        let request_hash = row
            .try_get::<Vec<u8>, _>("request_hash")
            .map_err(map_sql_error)?
            .try_into()
            .map_err(|_| invalid_record())?;
        let payload: Value = row.try_get("result_payload").map_err(map_sql_error)?;
        let stored = StoredCommand {
            command_id,
            command_kind: row.try_get("command_kind").map_err(map_sql_error)?,
            aggregate_id: row.try_get("aggregate_id").map_err(map_sql_error)?,
            request_hash: RequestHash::new(request_hash),
            result_code: row.try_get("result_code").map_err(map_sql_error)?,
            result_payload: serde_json::to_vec(&payload).map_err(|_| invalid_record())?,
        };
        stored.validate().map_err(CompetitionTransactionError::from)?;
        Ok(Some(stored))
    }

    pub(crate) async fn insert_command_inner(
        &mut self,
        command: StoredCommand,
    ) -> Result<(), CompetitionTransactionError> {
        command.validate().map_err(CompetitionTransactionError::from)?;
        let payload: Value =
            serde_json::from_slice(&command.result_payload).map_err(|_| invalid_record())?;
        let inserted = {
            let transaction = self.transaction_mut()?;
            sqlx::query(
                r#"INSERT INTO public.auction_commands
                    (command_id, command_kind, aggregate_id, request_hash, result_code, result_payload)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (command_id) DO NOTHING"#,
            )
            .bind(command.command_id.as_uuid())
            .bind(&command.command_kind)
            .bind(command.aggregate_id)
            .bind(command.request_hash.as_bytes().to_vec())
            .bind(&command.result_code)
            .bind(payload)
            .execute(&mut **transaction)
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

    pub(crate) async fn load_for_update_inner(
        &mut self,
        lot_id: LotId,
    ) -> Result<CompetitionSnapshot, CompetitionTransactionError> {
        let row = {
            let transaction = self.transaction_mut()?;
            sqlx::query(
                r#"SELECT
                    l.start_price, l.current_price, l.leader_user_id, l.leader_proxy_last,
                    l.effective_end_at, l.state_version, l.last_proxy_sequence,
                    l.slow_generation, l.slow_authorization_id, l.slow_due_at, l.material,
                    a.default_start_price, a.soft_close_window_seconds,
                    a.soft_close_extension_seconds, a.cancel_cutoff_seconds,
                    a.proxy_slow_interval_seconds,
                    COALESCE((a.policy_snapshot ->> 'bid_increment_ppm')::bigint, 0)
                        AS policy_bid_increment_ppm,
                    COALESCE((a.policy_snapshot ->> 'minimum_increment')::bigint, 30000)
                        AS policy_minimum_increment,
                    COALESCE((a.policy_snapshot ->> 'proxy_enabled')::boolean, true)
                        AS policy_proxy_enabled,
                    COALESCE(a.policy_snapshot ->> 'proxy_default_mode', 'fast')
                        AS policy_proxy_default_mode,
                    COALESCE((a.policy_snapshot ->> 'proxy_max_outbid_uses')::bigint, 0)
                        AS policy_proxy_max_outbid_uses,
                    COALESCE((a.policy_snapshot ->> 'final_phase_lead_seconds')::bigint, 60)
                        AS policy_final_phase_lead_seconds
                 FROM public.lots AS l
                 INNER JOIN public.auctions AS a ON a.id = l.auction_id
                 WHERE l.id = $1 AND l.status = 'live'
                 FOR UPDATE OF l"#,
            )
            .bind(lot_id.as_uuid())
            .fetch_optional(&mut **transaction)
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
                .try_get::<Option<Uuid>, _>("leader_user_id")
                .map_err(map_sql_error)?
                .map(auction_domain::UserId::from_uuid),
            leader_proxy_last: row.try_get("leader_proxy_last").map_err(map_sql_error)?,
            material: row.try_get("material").map_err(map_sql_error)?,
            effective_end_at: row
                .try_get::<Option<DateTime<Utc>>, _>("effective_end_at")
                .map_err(map_sql_error)?
                .ok_or_else(|| invalid_state("lot 缺少有效结束时间"))?,
            state_version: row
                .try_get::<i64, _>("state_version")
                .map_err(map_sql_error)?
                .try_into()
                .map_err(|_| invalid_state("lot 状态版本超出领域范围"))?,
            last_proxy_sequence: row
                .try_get::<i64, _>("last_proxy_sequence")
                .map_err(map_sql_error)?
                .try_into()
                .map_err(|_| invalid_state("代理序号超出领域范围"))?,
            slow_generation: row
                .try_get::<i64, _>("slow_generation")
                .map_err(map_sql_error)?
                .try_into()
                .map_err(|_| invalid_state("慢速代理世代超出领域范围"))?,
            slow_authorization_id: row
                .try_get::<Option<Uuid>, _>("slow_authorization_id")
                .map_err(map_sql_error)?
                .map(ProxyAuthorizationId::from_uuid),
            slow_due_at: row.try_get("slow_due_at").map_err(map_sql_error)?,
        };
        let active_proxy = {
            let transaction = self.transaction_mut()?;
            let row = sqlx::query(
                r#"SELECT authorization_id, lot_id, user_id, creation_command_id,
                    authorization_sequence, usage_number, maximum_amount, mode, status, state_version
                 FROM public.bid_proxy_authorizations
                 WHERE lot_id = $1 AND status IN ('active', 'final_fast')
                 ORDER BY authorization_sequence DESC
                 LIMIT 1
                 FOR UPDATE"#,
            )
            .bind(lot_id.as_uuid())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(map_sql_error)?;
            row.map(proxy_from_row).transpose()?
        };

        rules.validate().map_err(|error| invalid_state(error.to_string().as_str()))?;
        state.validate().map_err(|error| invalid_state(error.to_string().as_str()))?;
        Ok(CompetitionSnapshot { rules, state, active_proxy })
    }

    /// lot 已由调用方锁定；在同一事务中读取历史最大使用次数，保证重入编号不会并发重复。
    async fn next_proxy_usage_number_inner(
        &mut self,
        lot_id: LotId,
        user_id: UserId,
    ) -> Result<u64, CompetitionTransactionError> {
        let next_usage: i64 = {
            let transaction = self.transaction_mut()?;
            sqlx::query_scalar(
                "SELECT COALESCE(MAX(usage_number), 0) + 1
                 FROM public.bid_proxy_authorizations
                 WHERE lot_id = $1 AND user_id = $2",
            )
            .bind(lot_id.as_uuid())
            .bind(user_id.as_uuid())
            .fetch_one(&mut **transaction)
            .await
            .map_err(map_sql_error)?
        };
        if next_usage <= 0 {
            return Err(invalid_state("代理使用次数超出数据库范围"));
        }
        u64::try_from(next_usage).map_err(|_| invalid_state("代理使用次数超出领域范围"))
    }

    pub(crate) async fn apply_plan_inner(
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
    transaction: &mut PostgresCompetitionTransaction,
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
        let transaction = transaction.transaction_mut()?;
        let result = sqlx::query(
            r#"UPDATE public.bid_proxy_authorizations
             SET maximum_amount = $2, mode = $3, status = $4, terminal_reason = $5,
                 state_version = $6, updated_at = CURRENT_TIMESTAMP
             WHERE authorization_id = $1 AND state_version = $7"#,
        )
        .bind(state.id.as_uuid())
        .bind(state.maximum_amount.value())
        .bind(proxy_mode(state.mode))
        .bind(proxy_status(state.status))
        .bind(reason.map(proxy_terminal_reason))
        .bind(i64_version(state.state_version, "代理状态版本")?)
        .bind(i64_version(previous_version, "代理前一版本")?)
        .execute(&mut **transaction)
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
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            r#"INSERT INTO public.bid_proxy_authorizations
                (authorization_id, lot_id, user_id, authorization_sequence, usage_number,
                 maximum_amount, mode, status, terminal_reason, state_version,
                 creation_command_id, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)"#,
        )
        .bind(state.id.as_uuid())
        .bind(state.lot_id.as_uuid())
        .bind(state.user_id.as_uuid())
        .bind(i64_version(state.authorization_sequence, "代理序号")?)
        .bind(i64_version(state.usage_number, "代理使用次数")?)
        .bind(state.maximum_amount.value())
        .bind(proxy_mode(state.mode))
        .bind(proxy_status(state.status))
        .bind(reason.map(proxy_terminal_reason))
        .bind(i64_version(state.state_version, "代理状态版本")?)
        .bind(state.creation_command_id.as_uuid())
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    }

    for transition in &plan.proxy_transitions {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            r#"INSERT INTO public.bid_proxy_authorization_transitions
                (authorization_id, transition_number, transition_kind, origin_command_id,
                 old_maximum_amount, new_maximum_amount, old_mode, new_mode, old_status,
                 new_status, reason, resulting_state_version, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, CURRENT_TIMESTAMP)"#,
        )
        .bind(transition.authorization_id.as_uuid())
        .bind(i64_version(transition.state_version, "代理转换序号")?)
        .bind(proxy_transition_kind(transition.kind))
        .bind(transition.command_id.as_uuid())
        .bind(transition.old_maximum.map(BidAmount::value))
        .bind(transition.new_maximum.value())
        .bind(transition.old_mode.map(proxy_mode))
        .bind(proxy_mode(transition.new_mode))
        .bind(transition.old_status.map(proxy_status))
        .bind(proxy_status(transition.new_status))
        .bind(transition.reason.map(proxy_terminal_reason))
        .bind(i64_version(transition.state_version, "代理转换版本")?)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    }
    Ok(())
}

async fn insert_bid(
    transaction: &mut PostgresCompetitionTransaction,
    bid: &AcceptedBid,
) -> Result<(), CompetitionTransactionError> {
    let transaction = transaction.transaction_mut()?;
    sqlx::query(
        r#"INSERT INTO public.bids
            (id, command_id, fact_index, lot_id, bidder_user_id, amount, mechanism,
             effective_end_at, accepted_state_version)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
    )
    .bind(bid.id.as_uuid())
    .bind(bid.command_id.as_uuid())
    .bind(i32_version(bid.fact_index, "竞价事实序号")?)
    .bind(bid.lot_id.as_uuid())
    .bind(bid.bidder_id.as_uuid())
    .bind(bid.amount.value())
    .bind(bid_mechanism(bid.mechanism))
    .bind(bid.effective_end_at)
    .bind(i64_version(bid.state_version, "竞价状态版本")?)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

async fn update_lot(
    transaction: &mut PostgresCompetitionTransaction,
    plan: &CompetitionPlan,
) -> Result<(), CompetitionTransactionError> {
    let previous_version = plan
        .final_lot
        .state_version
        .checked_sub(plan.public_bids.len() as u64)
        .ok_or_else(|| invalid_state("lot 状态版本回退失败"))?;
    let transaction = transaction.transaction_mut()?;
    let result = sqlx::query(
        r#"UPDATE public.lots
         SET current_price = $2, leader_user_id = $3,
             leader_since = CASE
                 WHEN leader_user_id IS DISTINCT FROM $3 THEN CURRENT_TIMESTAMP
                 WHEN $3 IS NULL THEN NULL
                 ELSE leader_since
             END,
             leader_proxy_last = $4, effective_end_at = $5,
             slow_authorization_id = $6, slow_due_at = $7, slow_generation = $8,
             last_proxy_sequence = $9, state_version = $10, updated_at = CURRENT_TIMESTAMP
         WHERE id = $1 AND state_version = $11"#,
    )
    .bind(plan.final_lot.lot_id.as_uuid())
    .bind(plan.final_lot.current_price.map(BidAmount::value))
    .bind(plan.final_lot.leader_id.map(auction_domain::UserId::as_uuid))
    .bind(plan.final_lot.leader_proxy_last)
    .bind(plan.final_lot.effective_end_at)
    .bind(plan.final_lot.slow_authorization_id.map(ProxyAuthorizationId::as_uuid))
    .bind(plan.final_lot.slow_due_at)
    .bind(i64_version(plan.final_lot.slow_generation, "慢速代理世代")?)
    .bind(i64_version(plan.final_lot.last_proxy_sequence, "代理序号")?)
    .bind(i64_version(plan.final_lot.state_version, "lot 状态版本")?)
    .bind(i64_version(previous_version, "lot 前一版本")?)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    if result.rows_affected() != 1 {
        return Err(CompetitionTransactionError::Failed("lot 状态版本与数据库不一致".to_owned()));
    }
    Ok(())
}

async fn insert_audit_for_bid(
    transaction: &mut PostgresCompetitionTransaction,
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
    let transaction = transaction.transaction_mut()?;
    sqlx::query(
        r#"INSERT INTO public.audit_records
            (id, actor_type, actor_id, action, entity_type, entity_id, result, request_id, source, metadata)
         VALUES ($1, 'public_user', $2, 'place_bid', 'lot', $3, 'success', $4, 'public', $5)"#,
    )
    .bind(Uuid::new_v4())
    .bind(bid.bidder_id.as_uuid().to_string())
    .bind(bid.lot_id.as_uuid().to_string())
    .bind(request_id)
    .bind(metadata)
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;
    Ok(())
}

async fn insert_audit_for_proxy_changes(
    transaction: &mut PostgresCompetitionTransaction,
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
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            r#"INSERT INTO public.audit_records
                (id, actor_type, actor_id, action, entity_type, entity_id, result, request_id, source, metadata)
             VALUES ($1, 'system', 'auction-application', 'proxy_transition',
                     'proxy_authorization', $2, 'success', $3, 'system', $4)"#,
        )
        .bind(Uuid::new_v4())
        .bind(transition.authorization_id.as_uuid().to_string())
        .bind(request_id)
        .bind(metadata)
        .execute(&mut **transaction)
        .await
        .record_audit_failure()
        .map_err(map_sql_error)?;
    }
    Ok(())
}

async fn insert_realtime_event(
    transaction: &mut PostgresCompetitionTransaction,
    bid: &AcceptedBid,
) -> Result<(), CompetitionTransactionError> {
    let transaction = transaction.transaction_mut()?;
    let auction_id: Uuid = sqlx::query_scalar("SELECT auction_id FROM public.lots WHERE id = $1")
        .bind(bid.lot_id.as_uuid())
        .fetch_one(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    let stream_key = format!("auction:{auction_id}");
    let payload = json!({
        "auction_id": auction_id,
        "lot_id": bid.lot_id.as_uuid(),
        "bid_id": bid.id.as_uuid(),
        "amount": bid.amount.value(),
        "effective_end": bid.effective_end_at,
        "state_version": bid.state_version,
        "fact_index": bid.fact_index,
        "mechanism": bid_mechanism_name(bid.mechanism),
    });
    sqlx::query(
        r#"INSERT INTO public.realtime_streams (stream_key) VALUES ($1)
         ON CONFLICT (stream_key) DO NOTHING"#,
    )
    .bind(&stream_key)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let sequence: i64 = sqlx::query_scalar(
        "SELECT next_sequence FROM public.realtime_streams WHERE stream_key = $1 FOR UPDATE",
    )
    .bind(&stream_key)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        r#"UPDATE public.realtime_streams SET next_sequence = next_sequence + 1,
         updated_at = CURRENT_TIMESTAMP WHERE stream_key = $1"#,
    )
    .bind(&stream_key)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        r#"INSERT INTO public.realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
             state_version, origin_command_id, origin_bid_id, origin_fact_index)
         VALUES ($1, $2, $3, 'lot.bid_accepted', 2, $4, $5, $6, $7, $8)"#,
    )
    .bind(Uuid::new_v4())
    .bind(&stream_key)
    .bind(sequence)
    .bind(payload)
    .bind(i64_version(bid.state_version, "实时事件状态版本")?)
    .bind(bid.command_id.as_uuid())
    .bind(bid.id.as_uuid())
    .bind(i32_version(bid.fact_index, "实时事件事实序号")?)
    .execute(&mut **transaction)
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

fn proxy_from_row(
    row: sqlx::postgres::PgRow,
) -> Result<ProxyAuthorization, CompetitionTransactionError> {
    let mode = parse_proxy_mode(row.try_get::<String, _>("mode").map_err(map_sql_error)?)?;
    let status = parse_proxy_status(row.try_get::<String, _>("status").map_err(map_sql_error)?)?;
    let authorization = ProxyAuthorization {
        id: ProxyAuthorizationId::from_uuid(
            row.try_get("authorization_id").map_err(map_sql_error)?,
        ),
        lot_id: LotId::from_uuid(row.try_get("lot_id").map_err(map_sql_error)?),
        user_id: auction_domain::UserId::from_uuid(row.try_get("user_id").map_err(map_sql_error)?),
        creation_command_id: auction_domain::CommandId::from_uuid(
            row.try_get("creation_command_id").map_err(map_sql_error)?,
        ),
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
    row: &sqlx::postgres::PgRow,
    column: &str,
    label: &str,
) -> Result<BidAmount, CompetitionTransactionError> {
    let value: i64 = row.try_get(column).map_err(map_sql_error)?;
    BidAmount::new(value).map_err(|_| invalid_state(label))
}

fn optional_bid_amount(
    row: &sqlx::postgres::PgRow,
    column: &str,
    label: &str,
) -> Result<Option<BidAmount>, CompetitionTransactionError> {
    let value: Option<i64> = row.try_get(column).map_err(map_sql_error)?;
    value.map(|value| BidAmount::new(value).map_err(|_| invalid_state(label))).transpose()
}

fn auction_rules_from_row(row: &PgRow) -> Result<AuctionRules, CompetitionTransactionError> {
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
        proxy_enabled: row.try_get("policy_proxy_enabled").map_err(map_sql_error)?,
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

fn i64_version(value: u64, label: &str) -> Result<i64, CompetitionTransactionError> {
    value.try_into().map_err(|_| invalid_state(label))
}

fn i32_version(value: u32, label: &str) -> Result<i32, CompetitionTransactionError> {
    value.try_into().map_err(|_| invalid_state(label))
}

fn policy_u32(value: i64, label: &str) -> Result<u32, CompetitionTransactionError> {
    if value < 0 {
        return Err(invalid_state(label));
    }
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

pub(crate) async fn set_isolation(
    transaction: Result<&mut PgTransaction, CompetitionTransactionError>,
    isolation: TransactionIsolation,
) -> Result<(), CompetitionTransactionError> {
    let transaction = transaction?;
    let statement = match isolation {
        TransactionIsolation::Serializable => "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
        TransactionIsolation::ReadCommitted => "SET TRANSACTION ISOLATION LEVEL READ COMMITTED",
    };
    sqlx::query(statement).execute(&mut **transaction).await.map_err(map_sql_error)?;
    Ok(())
}

pub(crate) fn map_sql_error(error: sqlx::Error) -> CompetitionTransactionError {
    if is_serialization_error(&error) {
        CompetitionTransactionError::SerializationConflict
    } else if is_unique_error(&error) {
        CompetitionTransactionError::Conflict
    } else {
        CompetitionTransactionError::Failed("PostgreSQL 竞价事务操作失败".to_owned())
    }
}

fn is_serialization_error(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(database_error) if database_error.code().as_deref() == Some("40001"))
}

fn is_unique_error(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(database_error) if database_error.code().as_deref() == Some("23505"))
}

fn invalid_record() -> CompetitionTransactionError {
    CompetitionTransactionError::CommandLedger(CommandLedgerError::InvalidRecord)
}

fn invalid_state(message: &str) -> CompetitionTransactionError {
    CompetitionTransactionError::Failed(message.to_owned())
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
        _ => Err(invalid_state("拍卖状态不在允许集合内")),
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
