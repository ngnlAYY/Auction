use std::future::Future;

use auction_application::{
    SettlementSnapshot, SettlementTransaction, SettlementTransactionError,
    SettlementTransactionFailure, SettlementTransactionRunner, StoredCommand, TransactionFuture,
    TransactionIsolation, TransactionPolicy,
};
use auction_domain::{
    AuctionId, CommandId, SettlementAmount, SettlementFact, SettlementMode,
    SettlementObligationKind, SettlementObligationState, SettlementPlan, SettlementPolicy,
    SettlementWorld, deterministic_uuid,
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use super::{PostgresCompetitionTransaction, PostgresStore};

/// PostgreSQL 结算事务 runner。结算与竞价都使用 Serializable，但锁集合和落表步骤保持独立。
#[allow(clippy::manual_async_fn)]
impl SettlementTransactionRunner for PostgresStore {
    type Transaction = PostgresCompetitionTransaction;

    fn run<T, E, F>(
        &self,
        policy: TransactionPolicy,
        operation: F,
    ) -> impl Future<Output = Result<T, E>> + Send
    where
        T: Send,
        E: SettlementTransactionFailure,
        F: for<'a> Fn(&'a mut Self::Transaction) -> TransactionFuture<'a, T, E> + Send,
    {
        async move {
            if let Err(error) = TransactionPolicy::new(policy.isolation, policy.max_attempts) {
                return Err(E::from(error));
            }

            for attempt in 0..policy.max_attempts {
                let transaction = match self.pool().begin().await {
                    Ok(transaction) => transaction,
                    Err(_) => {
                        return Err(E::from(SettlementTransactionError::Failed(
                            "PostgreSQL 结算事务无法开始".to_owned(),
                        )));
                    }
                };
                let mut transaction = PostgresCompetitionTransaction::new(transaction);

                if let Err(error) = set_isolation(&mut transaction, policy.isolation).await {
                    if matches!(error, SettlementTransactionError::SerializationConflict)
                        && attempt + 1 < policy.max_attempts
                    {
                        continue;
                    }
                    return Err(E::from(error));
                }

                match operation(&mut transaction).await {
                    Ok(value) => match transaction.commit().await {
                        Ok(()) => return Ok(value),
                        Err(_) if attempt + 1 < policy.max_attempts => continue,
                        Err(_) => {
                            return Err(E::from(SettlementTransactionError::Failed(
                                "PostgreSQL 结算事务提交失败".to_owned(),
                            )));
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

            Err(E::from(SettlementTransactionError::SerializationConflict))
        }
    }
}

#[allow(clippy::manual_async_fn)]
impl SettlementTransaction for PostgresCompetitionTransaction {
    fn find_command(
        &mut self,
        command_id: CommandId,
    ) -> impl Future<Output = Result<Option<StoredCommand>, SettlementTransactionError>> + Send
    {
        async move { self.find_command_inner(command_id).await.map_err(map_competition_error) }
    }

    fn load_for_update(
        &mut self,
        auction_id: AuctionId,
    ) -> impl Future<Output = Result<SettlementSnapshot, SettlementTransactionError>> + Send {
        async move { self.load_settlement_for_update(auction_id).await }
    }

    fn insert_command(
        &mut self,
        command: StoredCommand,
    ) -> impl Future<Output = Result<(), SettlementTransactionError>> + Send {
        async move { self.insert_command_inner(command).await.map_err(map_competition_error) }
    }

    fn apply_settlement(
        &mut self,
        auction_id: AuctionId,
        command_id: CommandId,
        plan: SettlementPlan,
    ) -> impl Future<Output = Result<(), SettlementTransactionError>> + Send {
        async move { self.apply_settlement_inner(auction_id, command_id, plan).await }
    }
}

async fn set_isolation(
    transaction: &mut PostgresCompetitionTransaction,
    isolation: TransactionIsolation,
) -> Result<(), SettlementTransactionError> {
    let statement = match isolation {
        TransactionIsolation::Serializable => "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
        TransactionIsolation::ReadCommitted => "SET TRANSACTION ISOLATION LEVEL READ COMMITTED",
    };
    let transaction = transaction.transaction_mut().map_err(map_competition_error)?;
    sqlx::query(statement).execute(&mut **transaction).await.map_err(map_sql_error)?;
    Ok(())
}

impl PostgresCompetitionTransaction {
    async fn load_settlement_for_update(
        &mut self,
        auction_id: AuctionId,
    ) -> Result<SettlementSnapshot, SettlementTransactionError> {
        let auction_row = {
            let transaction = self.transaction_mut().map_err(map_competition_error)?;
            sqlx::query(
                r#"SELECT
                       a.world_id,
                       a.state_version AS auction_state_version,
                       a.scheduled_open_at,
                       COALESCE(
                           a.effective_end_at,
                           a.scheduled_end_at,
                           a.scheduled_open_at + interval '24 hours'
                       ) AS scheduled_end_at,
                       a.policy_snapshot,
                       p.balance AS opening_pool,
                       p.state_version AS opening_pool_version
                   FROM public.auctions AS a
                   INNER JOIN public.settlement_pools AS p ON p.world_id = a.world_id
                   WHERE a.id = $1 AND a.status IN ('settling', 'settled')
                   FOR UPDATE OF a, p"#,
            )
            .bind(auction_id.as_uuid())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(map_sql_error)?
        };
        let Some(auction_row) = auction_row else {
            return Err(SettlementTransactionError::NotFound);
        };

        let world =
            settlement_world(auction_row.try_get::<String, _>("world_id").map_err(map_sql_error)?)?;
        let policy_snapshot: Value =
            auction_row.try_get("policy_snapshot").map_err(map_sql_error)?;
        let policy = parse_settlement_policy(&policy_snapshot)?;
        let scheduled_open_at: DateTime<Utc> =
            auction_row.try_get("scheduled_open_at").map_err(map_sql_error)?;
        let scheduled_end_at: DateTime<Utc> =
            auction_row.try_get("scheduled_end_at").map_err(map_sql_error)?;
        let auction_state_version = positive_u64(
            auction_row.try_get::<i64, _>("auction_state_version").map_err(map_sql_error)?,
            "拍卖状态版本",
        )?;
        let opening_pool = SettlementAmount::new(
            auction_row.try_get::<i64, _>("opening_pool").map_err(map_sql_error)?,
        )
        .map_err(|_| invalid("结算池余额无效"))?;
        let opening_pool_version = positive_u64(
            auction_row.try_get::<i64, _>("opening_pool_version").map_err(map_sql_error)?,
            "结算池状态版本",
        )?;

        let rows = {
            let transaction = self.transaction_mut().map_err(map_competition_error)?;
            sqlx::query(
                r#"SELECT
                       l.id,
                       l.current_price,
                       l.leader_user_id,
                       l.leader_since,
                       l.effective_end_at,
                       l.equipment_id,
                       l.submission_id,
                       COALESCE(
                           l.effective_end_at,
                           a.effective_end_at,
                           a.scheduled_end_at,
                           a.scheduled_open_at + interval '24 hours'
                       ) AS fact_end_at,
                       submission.seller
                   FROM public.lots AS l
                   INNER JOIN public.auctions AS a ON a.id = l.auction_id
                   LEFT JOIN public.auction_submissions AS submission
                       ON submission.id = l.submission_id
                   WHERE l.auction_id = $1
                     AND l.status IN ('sold', 'unsold', 'withdrawn')
                   ORDER BY l.lot_number, l.id
                   FOR UPDATE OF l"#,
            )
            .bind(auction_id.as_uuid())
            .fetch_all(&mut **transaction)
            .await
            .map_err(map_sql_error)?
        };

        let mut facts = Vec::with_capacity(rows.len());
        for row in rows {
            let leader_id = row
                .try_get::<Option<Uuid>, _>("leader_user_id")
                .map_err(map_sql_error)?
                .map(auction_domain::UserId::from_uuid);
            let end_price = row
                .try_get::<Option<i64>, _>("current_price")
                .map_err(map_sql_error)?
                .map(|value| SettlementAmount::new(value).map_err(|_| invalid("成交金额无效")))
                .transpose()?;
            let seller_identity: String = row
                .try_get::<Option<String>, _>("seller")
                .map_err(map_sql_error)?
                .ok_or_else(|| invalid("结算 lot 缺少卖方身份"))?;
            let fact_end_at: DateTime<Utc> = row.try_get("fact_end_at").map_err(map_sql_error)?;
            let fact = SettlementFact {
                lot_id: row
                    .try_get::<Uuid, _>("id")
                    .map(auction_domain::LotId::from_uuid)
                    .map_err(map_sql_error)?,
                world,
                end_price: leader_id.and(end_price),
                leader_id,
                leader_since: row.try_get("leader_since").map_err(map_sql_error)?,
                effective_end_at: fact_end_at,
                seller_identity,
                equipment_id: row
                    .try_get::<Uuid, _>("equipment_id")
                    .map(auction_domain::EquipmentId::from_uuid)
                    .map_err(map_sql_error)?,
                submission_id: row
                    .try_get::<Option<Uuid>, _>("submission_id")
                    .map_err(map_sql_error)?
                    .map(auction_domain::SubmissionId::from_uuid),
            };
            facts.push(fact);
        }

        Ok(SettlementSnapshot {
            auction_id,
            auction_state_version,
            policy,
            scheduled_open_at,
            scheduled_end_at,
            opening_pool,
            opening_pool_version,
            facts,
        })
    }

    async fn apply_settlement_inner(
        &mut self,
        auction_id: AuctionId,
        command_id: CommandId,
        plan: SettlementPlan,
    ) -> Result<(), SettlementTransactionError> {
        for line in &plan.lines {
            line.validate().map_err(|_| invalid("结算明细校验失败"))?;
        }
        for obligation in &plan.obligations {
            obligation.validate().map_err(|_| invalid("结算义务校验失败"))?;
        }

        let (world_id, auction_state_version) = {
            let transaction = self.transaction_mut().map_err(map_competition_error)?;
            let row = sqlx::query(
                r#"SELECT world_id, state_version
                   FROM public.auctions
                   WHERE id = $1 AND status = 'settling'
                   FOR UPDATE"#,
            )
            .bind(auction_id.as_uuid())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(map_sql_error)?
            .ok_or(SettlementTransactionError::NotFound)?;
            (
                row.try_get::<String, _>("world_id").map_err(map_sql_error)?,
                positive_u64(
                    row.try_get::<i64, _>("state_version").map_err(map_sql_error)?,
                    "拍卖状态版本",
                )?,
            )
        };
        let current_pool_version = {
            let transaction = self.transaction_mut().map_err(map_competition_error)?;
            let row = sqlx::query(
                r#"SELECT balance, state_version
                   FROM public.settlement_pools
                   WHERE world_id = $1
                   FOR UPDATE"#,
            )
            .bind(&world_id)
            .fetch_one(&mut **transaction)
            .await
            .map_err(map_sql_error)?;
            let current_balance: i64 = row.try_get("balance").map_err(map_sql_error)?;
            if current_balance != plan.opening_pool.value() {
                return Err(invalid("结算池余额与结算计划不一致"));
            }
            positive_u64(row.try_get("state_version").map_err(map_sql_error)?, "结算池状态版本")?
        };
        let closing_pool_version =
            current_pool_version.checked_add(1).ok_or_else(|| invalid("结算池状态版本溢出"))?;

        {
            let transaction = self.transaction_mut().map_err(map_competition_error)?;
            let result = sqlx::query(
                r#"UPDATE public.settlement_pools
                   SET balance = $2, state_version = $3, updated_at = CURRENT_TIMESTAMP
                   WHERE world_id = $1 AND balance = $4 AND state_version = $5"#,
            )
            .bind(&world_id)
            .bind(plan.closing_pool.value())
            .bind(i64_version(closing_pool_version, "结算池状态版本")?)
            .bind(plan.opening_pool.value())
            .bind(i64_version(current_pool_version, "结算池状态版本")?)
            .execute(&mut **transaction)
            .await
            .map_err(map_sql_error)?;
            if result.rows_affected() != 1 {
                return Err(invalid("结算池版本与数据库不一致"));
            }

            sqlx::query(
                r#"INSERT INTO public.auction_settlements
                       (auction_id, command_id, policy_version, policy_mode, pool_enabled,
                        opening_pool_balance, closing_pool_balance, tax_total, discount_demand,
                        compensation_total, opening_pool_version, closing_pool_version, settled_at)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, CURRENT_TIMESTAMP)"#,
            )
            .bind(auction_id.as_uuid())
            .bind(command_id.as_uuid())
            .bind(i32_version(plan.policy_version, "结算策略版本")?)
            .bind(settlement_mode(plan.policy_mode))
            .bind(plan.compensation_pool_enabled)
            .bind(plan.opening_pool.value())
            .bind(plan.closing_pool.value())
            .bind(plan.tax_total.value())
            .bind(plan.discount_demand.value())
            .bind(plan.compensation_total.value())
            .bind(i64_version(current_pool_version, "结算池状态版本")?)
            .bind(i64_version(closing_pool_version, "结算池状态版本")?)
            .execute(&mut **transaction)
            .await
            .map_err(map_sql_error)?;
        }

        insert_lot_settlements(self, &plan).await?;
        insert_compensation_ledger(self, command_id, &plan).await?;
        insert_obligations(self, &plan).await?;
        insert_settlement_audit(self, auction_id, command_id, auction_state_version).await?;

        {
            let transaction = self.transaction_mut().map_err(map_competition_error)?;
            let next_version =
                auction_state_version.checked_add(1).ok_or_else(|| invalid("拍卖状态版本溢出"))?;
            let result = sqlx::query(
                r#"UPDATE public.auctions
                   SET status = 'settled', settled_at = CURRENT_TIMESTAMP,
                       state_version = $2, updated_at = CURRENT_TIMESTAMP
                   WHERE id = $1 AND status = 'settling' AND state_version = $3"#,
            )
            .bind(auction_id.as_uuid())
            .bind(i64_version(next_version, "拍卖状态版本")?)
            .bind(i64_version(auction_state_version, "拍卖状态版本")?)
            .execute(&mut **transaction)
            .await
            .map_err(map_sql_error)?;
            if result.rows_affected() != 1 {
                return Err(invalid("拍卖状态版本与数据库不一致"));
            }
        }

        insert_settlement_realtime(self, auction_id, command_id, auction_state_version + 1).await
    }
}

async fn insert_lot_settlements(
    transaction: &mut PostgresCompetitionTransaction,
    plan: &SettlementPlan,
) -> Result<(), SettlementTransactionError> {
    for line in &plan.lines {
        let transaction = transaction.transaction_mut().map_err(map_competition_error)?;
        sqlx::query(
            r#"INSERT INTO public.lot_settlements
                   (lot_id, end_price, listed_price, buyer_amount, seller_amount,
                    tax_amount, compensation_amount, hold_ppm, adjustment_ppm, adjustment_metadata)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"#,
        )
        .bind(line.lot_id.as_uuid())
        .bind(line.end_price.value())
        .bind(line.listed_price.value())
        .bind(line.buyer_amount.value())
        .bind(line.seller_amount.value())
        .bind(line.tax_amount.value())
        .bind(line.compensation_amount.value())
        .bind(line.hold_ppm)
        .bind(line.adjustment_ppm)
        .bind(json!({
            "policy": settlement_mode(plan.policy_mode),
            "policy_version": plan.policy_version,
        }))
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    }
    Ok(())
}

async fn insert_compensation_ledger(
    transaction: &mut PostgresCompetitionTransaction,
    command_id: CommandId,
    plan: &SettlementPlan,
) -> Result<(), SettlementTransactionError> {
    let mut sequence: i32 = 1;
    let mut balance = plan.opening_pool.value() as i128;
    insert_ledger_entry(
        transaction,
        command_id,
        sequence,
        "opening",
        plan.opening_pool.value(),
        balance,
        None,
    )
    .await?;
    sequence += 1;

    for line in &plan.lines {
        if line.tax_amount.value() > 0 {
            balance = balance
                .checked_add(i128::from(line.tax_amount.value()))
                .ok_or_else(|| invalid("补偿池账本余额溢出"))?;
            insert_ledger_entry(
                transaction,
                command_id,
                sequence,
                "tax",
                line.tax_amount.value(),
                balance,
                Some(line.lot_id),
            )
            .await?;
            sequence += 1;
        }
        if line.compensation_amount.value() > 0 {
            balance = balance
                .checked_sub(i128::from(line.compensation_amount.value()))
                .ok_or_else(|| invalid("补偿池账本余额下溢"))?;
            insert_ledger_entry(
                transaction,
                command_id,
                sequence,
                "compensation",
                -line.compensation_amount.value(),
                balance,
                Some(line.lot_id),
            )
            .await?;
            sequence += 1;
        }
    }

    let closing = i128::from(plan.closing_pool.value());
    if balance != closing {
        return Err(invalid("补偿池账本与结算计划不一致"));
    }
    balance = balance.checked_sub(closing).ok_or_else(|| invalid("补偿池关闭账本下溢"))?;
    insert_ledger_entry(
        transaction,
        command_id,
        sequence,
        "closing",
        -plan.closing_pool.value(),
        balance,
        None,
    )
    .await
}

async fn insert_ledger_entry(
    transaction: &mut PostgresCompetitionTransaction,
    command_id: CommandId,
    sequence: i32,
    movement_kind: &str,
    amount: i64,
    balance_after: i128,
    lot_id: Option<auction_domain::LotId>,
) -> Result<(), SettlementTransactionError> {
    let balance_after = i64::try_from(balance_after).map_err(|_| invalid("补偿池账本余额溢出"))?;
    if balance_after < 0 {
        return Err(invalid("补偿池账本余额不能为负"));
    }
    let id = deterministic_uuid(command_id, &format!("settlement-ledger:{sequence}"), 1);
    let transaction = transaction.transaction_mut().map_err(map_competition_error)?;
    sqlx::query(
        r#"INSERT INTO public.compensation_ledger
               (id, auction_id, lot_id, entry_sequence, movement_kind, amount, balance_after)
           VALUES ($1, (SELECT auction_id FROM public.auction_commands WHERE command_id = $2),
                   $3, $4, $5, $6, $7)"#,
    )
    .bind(id)
    .bind(command_id.as_uuid())
    .bind(lot_id.map(|id| id.as_uuid()))
    .bind(sequence)
    .bind(movement_kind)
    .bind(amount)
    .bind(balance_after)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

async fn insert_obligations(
    transaction: &mut PostgresCompetitionTransaction,
    plan: &SettlementPlan,
) -> Result<(), SettlementTransactionError> {
    for obligation in &plan.obligations {
        let transaction = transaction.transaction_mut().map_err(map_competition_error)?;
        sqlx::query(
            r#"INSERT INTO public.settlement_obligations
                   (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
                    obligation_kind, business_state, state_version, recipient_user_id,
                    recipient_game_identity, amount, carrier_credit, equipment_id, submission_id,
                    prerequisite_obligation_id)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NULLIF($11, ''),
                       $12, $13, $14, $15, $16)"#,
        )
        .bind(obligation.id.as_uuid())
        .bind(obligation.auction_id.as_uuid())
        .bind(obligation.lot_id.as_uuid())
        .bind(settlement_world_name(obligation.world))
        .bind(obligation.origin_command_id.as_uuid())
        .bind(i32_version(obligation.policy_version, "义务策略版本")?)
        .bind(obligation_kind(obligation.kind))
        .bind(obligation_state(obligation.state))
        .bind(i64_version(obligation.state_version, "义务状态版本")?)
        .bind(obligation.recipient_user_id.map(|id| id.as_uuid()))
        .bind(&obligation.recipient_game_identity)
        .bind(obligation.amount.value())
        .bind(i64::from(obligation.carrier_credit))
        .bind(obligation.equipment_id.as_uuid())
        .bind(obligation.submission_id.map(|id| id.as_uuid()))
        .bind(obligation.prerequisite_id.map(|id| id.as_uuid()))
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;

        let operation = match obligation.kind {
            SettlementObligationKind::UnsoldReturn | SettlementObligationKind::WithdrawnReturn => {
                "return"
            }
            _ => "send",
        };
        sqlx::query(
            r#"INSERT INTO public.settlement_obligation_fulfillments
                   (obligation_id, execution_state, dispatch_world, delivery_operation)
               VALUES ($1, 'pending', $2, $3)"#,
        )
        .bind(obligation.id.as_uuid())
        .bind(settlement_world_name(obligation.world))
        .bind(operation)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    }
    Ok(())
}

async fn insert_settlement_audit(
    transaction: &mut PostgresCompetitionTransaction,
    auction_id: AuctionId,
    command_id: CommandId,
    state_version: u64,
) -> Result<(), SettlementTransactionError> {
    let transaction = transaction.transaction_mut().map_err(map_competition_error)?;
    sqlx::query(
        r#"INSERT INTO public.audit_records
               (id, actor_type, actor_id, action, entity_type, entity_id, result, request_id, source, metadata)
           VALUES ($1, 'worker', $2, 'settle_auction', 'auction', $3, 'success', $2, 'worker', $4)"#,
    )
    .bind(deterministic_uuid(command_id, "settlement-audit", 1))
    .bind(command_id.to_string())
    .bind(auction_id.to_string())
    .bind(json!({ "state_version": state_version }))
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

async fn insert_settlement_realtime(
    transaction: &mut PostgresCompetitionTransaction,
    auction_id: AuctionId,
    command_id: CommandId,
    state_version: u64,
) -> Result<(), SettlementTransactionError> {
    let stream_key = format!("auction:{auction_id}");
    let event_id = deterministic_uuid(command_id, "settlement-realtime-event", 1);
    let transaction = transaction.transaction_mut().map_err(map_competition_error)?;
    sqlx::query(
        r#"INSERT INTO public.realtime_streams (stream_key)
           VALUES ($1)
           ON CONFLICT (stream_key) DO NOTHING"#,
    )
    .bind(&stream_key)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let row = sqlx::query(
        r#"UPDATE public.realtime_streams
           SET next_sequence = next_sequence + 1, updated_at = CURRENT_TIMESTAMP
           WHERE stream_key = $1
           RETURNING next_sequence - 1 AS stream_sequence"#,
    )
    .bind(&stream_key)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let stream_sequence: i64 = row.try_get("stream_sequence").map_err(map_sql_error)?;
    let counts = sqlx::query(
        r#"SELECT
               COUNT(*) FILTER (WHERE status = 'sold')::bigint AS sold_count,
               COUNT(*) FILTER (WHERE status = 'unsold')::bigint AS unsold_count
           FROM public.lots
           WHERE auction_id = $1"#,
    )
    .bind(auction_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let sold_count: i64 = counts.try_get("sold_count").map_err(map_sql_error)?;
    let unsold_count: i64 = counts.try_get("unsold_count").map_err(map_sql_error)?;
    sqlx::query(
        r#"INSERT INTO public.realtime_events
               (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
                state_version, origin_command_id)
           VALUES ($1, $2, $3, 'auction.settled', 1, $4, $5, $6)"#,
    )
    .bind(event_id)
    .bind(&stream_key)
    .bind(stream_sequence)
    .bind(json!({
        "auction_id": auction_id,
        "sold_count": sold_count,
        "unsold_count": unsold_count,
        "state_version": state_version,
    }))
    .bind(i64_version(state_version, "拍卖状态版本")?)
    .bind(command_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

fn parse_settlement_policy(
    snapshot: &Value,
) -> Result<SettlementPolicy, SettlementTransactionError> {
    let settlement = snapshot
        .get("settlement")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("拍卖策略快照缺少 settlement"))?;
    let version = json_u32(settlement.get("version"), "结算策略版本")?;
    let mode = match settlement.get("mode").and_then(Value::as_str) {
        Some("legacy_zero_v0") => SettlementMode::LegacyZeroV0,
        Some("hold_curve_v1") => SettlementMode::HoldCurveV1,
        _ => return Err(invalid("结算策略模式未知")),
    };
    let compensation_pool_enabled = settlement
        .get("compensation_pool_enabled")
        .and_then(Value::as_bool)
        .ok_or_else(|| invalid("补偿池策略字段无效"))?;
    let isekai_carrier_credit =
        json_u32(settlement.get("isekai_carrier_credit"), "isekai carrier credit")?;
    let anchors = settlement
        .get("anchors")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("结算锚点字段无效"))?
        .iter()
        .map(|anchor| {
            let object = anchor.as_object().ok_or_else(|| invalid("结算锚点不是对象"))?;
            Ok(auction_domain::AdjustmentAnchor {
                hold_ppm: json_i64(object.get("hold_ppm"), "结算锚点 hold_ppm")?,
                adjustment_ppm: json_i64(object.get("adjustment_ppm"), "结算锚点 adjustment_ppm")?,
            })
        })
        .collect::<Result<Vec<_>, SettlementTransactionError>>()?;
    let policy = SettlementPolicy {
        version,
        mode,
        compensation_pool_enabled,
        isekai_carrier_credit,
        anchors,
    };
    policy.validate().map_err(|_| invalid("结算策略快照校验失败"))?;
    Ok(policy)
}

fn settlement_world(value: String) -> Result<SettlementWorld, SettlementTransactionError> {
    match value.as_str() {
        "main" => Ok(SettlementWorld::Main),
        "isekai" => Ok(SettlementWorld::Isekai),
        _ => Err(invalid("世界标识不支持结算")),
    }
}

fn settlement_world_name(world: SettlementWorld) -> &'static str {
    match world {
        SettlementWorld::Main => "main",
        SettlementWorld::Isekai => "isekai",
    }
}

fn settlement_mode(mode: SettlementMode) -> &'static str {
    match mode {
        SettlementMode::LegacyZeroV0 => "legacy_zero_v0",
        SettlementMode::HoldCurveV1 => "hold_curve_v1",
    }
}

fn obligation_kind(kind: SettlementObligationKind) -> &'static str {
    match kind {
        SettlementObligationKind::WinnerEquipmentCod => "winner_equipment_cod",
        SettlementObligationKind::WinnerPaymentCarrierCod => "winner_payment_carrier_cod",
        SettlementObligationKind::WinnerEquipmentHandoff => "winner_equipment_handoff",
        SettlementObligationKind::SellerPayout => "seller_payout",
        SettlementObligationKind::BuyerRefund => "buyer_refund",
        SettlementObligationKind::UnsoldReturn => "unsold_return",
        SettlementObligationKind::WithdrawnReturn => "withdrawn_return",
    }
}

fn obligation_state(state: SettlementObligationState) -> &'static str {
    match state {
        SettlementObligationState::Ready => "ready",
        SettlementObligationState::Blocked => "blocked",
        SettlementObligationState::Contingent => "contingent",
    }
}

fn json_u32(value: Option<&Value>, label: &str) -> Result<u32, SettlementTransactionError> {
    let value = value.and_then(Value::as_i64).ok_or_else(|| invalid(label))?;
    u32::try_from(value).map_err(|_| invalid(label))
}

fn json_i64(value: Option<&Value>, label: &str) -> Result<i64, SettlementTransactionError> {
    value.and_then(Value::as_i64).ok_or_else(|| invalid(label))
}

fn positive_u64(value: i64, label: &str) -> Result<u64, SettlementTransactionError> {
    if value <= 0 {
        return Err(invalid(label));
    }
    u64::try_from(value).map_err(|_| invalid(label))
}

fn i64_version(value: u64, label: &str) -> Result<i64, SettlementTransactionError> {
    i64::try_from(value).map_err(|_| invalid(label))
}

fn i32_version(value: u32, label: &str) -> Result<i32, SettlementTransactionError> {
    i32::try_from(value).map_err(|_| invalid(label))
}

fn invalid(message: &str) -> SettlementTransactionError {
    SettlementTransactionError::Failed(message.to_owned())
}

fn map_competition_error(
    error: auction_application::CompetitionTransactionError,
) -> SettlementTransactionError {
    match error {
        auction_application::CompetitionTransactionError::NotFound => {
            SettlementTransactionError::NotFound
        }
        auction_application::CompetitionTransactionError::Conflict => {
            SettlementTransactionError::Conflict
        }
        auction_application::CompetitionTransactionError::StaleState
        | auction_application::CompetitionTransactionError::InvalidState => {
            SettlementTransactionError::Failed("竞价事务中的管理状态不允许当前结算操作".to_owned())
        }
        auction_application::CompetitionTransactionError::Failed(message) => {
            SettlementTransactionError::Failed(message)
        }
        auction_application::CompetitionTransactionError::SerializationConflict => {
            SettlementTransactionError::SerializationConflict
        }
        auction_application::CompetitionTransactionError::CommandLedger(error) => {
            SettlementTransactionError::CommandLedger(error)
        }
    }
}

fn map_sql_error(error: sqlx::Error) -> SettlementTransactionError {
    let error = super::competition::map_sql_error(error);
    map_competition_error(error)
}
