//! SQLite 竞价、代理和实时事件事务适配。
//!
//! 该文件只承载 SQLite Lite 竞价事务的单一职责，写事务边界仍由父模块统一装配。

use super::mapping::*;
use super::*;

impl SqliteCompetitionTransaction {
    pub(crate) async fn find_command_inner(
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

    pub(crate) async fn insert_command_inner(
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

    pub(crate) async fn load_for_update_inner(
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
    pub(crate) async fn next_proxy_usage_number_inner(
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
    .record_audit_failure()
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
        .record_audit_failure()
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
