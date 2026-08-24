//! PostgreSQL 竞价、代理和实时事件事务适配。
//!
//! 该文件只承载 PostgreSQL 竞价事务的单一职责，事务边界仍由父模块统一装配。

use super::mapping::*;
use super::*;

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
    pub(crate) async fn next_proxy_usage_number_inner(
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
