//! SQLite 竞价事务的锁内 lot 快照与代理使用次数读取。

use sqlx::Row;

use super::super::mapping::*;
use super::super::*;

impl SqliteCompetitionTransaction {
    pub(crate) async fn current_time_inner(
        &mut self,
    ) -> Result<DateTime<Utc>, CompetitionTransactionError> {
        let value: String = sqlx::query_scalar("SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now')")
            .fetch_one(&mut *self.connection_mut()?)
            .await
            .map_err(map_sql_error)?;
        parse_timestamp(value, "SQLite 数据库当前时间")
    }

    pub(crate) async fn load_for_update_inner(
        &mut self,
        lot_id: LotId,
        now: DateTime<Utc>,
    ) -> Result<CompetitionSnapshot, CompetitionTransactionError> {
        let row = {
            let connection = self.connection_mut()?;
            sqlx::query(
                "SELECT
                    l.start_price, l.current_price, l.leader_user_id, l.leader_proxy_last,
                    l.effective_end_at, l.state_version, l.last_proxy_sequence,
                    l.slow_generation, l.slow_authorization_id, l.slow_due_at, l.material,
                    l.status AS lot_status, a.status AS auction_status, a.scheduled_open_at,
                    w.enabled AS world_enabled,
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
                 INNER JOIN worlds AS w ON w.id = a.world_id
                 WHERE l.id = ?",
            )
            .bind(uuid_blob(lot_id.as_uuid()))
            .fetch_optional(&mut *connection)
            .await
            .map_err(map_sql_error)?
        };
        let Some(row) = row else {
            return Err(CompetitionTransactionError::NotFound);
        };

        let world_enabled = row.try_get::<i64, _>("world_enabled").map_err(map_sql_error)? != 0;
        let auction_status =
            parse_auction_status(row.try_get("auction_status").map_err(map_sql_error)?)?;
        let lot_status: String = row.try_get("lot_status").map_err(map_sql_error)?;
        let scheduled_open_at = parse_timestamp(
            row.try_get::<String, _>("scheduled_open_at").map_err(map_sql_error)?,
            "拍卖计划开放时间",
        )?;
        let effective_end_at = row
            .try_get::<Option<String>, _>("effective_end_at")
            .map_err(map_sql_error)?
            .map(|value| parse_timestamp(value, "lot 有效结束时间"))
            .transpose()?;
        if !world_enabled
            || !matches!(auction_status, AuctionStatus::Live | AuctionStatus::Final)
            || lot_status != "live"
            || now < scheduled_open_at
        {
            return Err(CompetitionTransactionError::InvalidState);
        }
        let effective_end_at = effective_end_at.ok_or(CompetitionTransactionError::InvalidState)?;
        if now >= effective_end_at {
            return Err(CompetitionTransactionError::InvalidState);
        }

        let rules = auction_rules_from_row(&row)?;
        let stored_start_price: i64 = row.try_get("start_price").map_err(map_sql_error)?;
        let start_price = BidAmount::from_persisted_positive(stored_start_price)
            .map_err(|_| invalid_state("Lite lot 起始价不是正数"))?;
        let stored_current_price: Option<i64> =
            row.try_get("current_price").map_err(map_sql_error)?;
        let leader_id = row
            .try_get::<Option<Vec<u8>>, _>("leader_user_id")
            .map_err(map_sql_error)?
            .map(|bytes| uuid_from_blob(bytes, "领先者标识"))
            .transpose()?
            .map(auction_domain::UserId::from_uuid);
        // Lite 同样可能读到旧版或缺陷版本写出的无领先者起始价哨兵；只归一化这一种
        // 可证明等价的形状，其余金额/领先者矛盾继续失败关闭。
        let current_price = match (stored_current_price, leader_id.is_some()) {
            (None, false) => None,
            (Some(current_price), false) if current_price == stored_start_price => None,
            (Some(current_price), true) => Some(
                BidAmount::from_persisted_positive(current_price)
                    .map_err(|_| invalid_state("Lite lot 当前价不是正数"))?,
            ),
            _ => return Err(invalid_state("Lite lot 当前价与领先者持久化投影不一致")),
        };
        let state = LotBidState {
            lot_id,
            start_price,
            current_price,
            leader_id,
            leader_proxy_last: row.try_get::<i64, _>("leader_proxy_last").map_err(map_sql_error)?
                != 0,
            material: row.try_get::<i64, _>("material").map_err(map_sql_error)? != 0,
            effective_end_at,
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
        Ok(CompetitionSnapshot { rules, auction_status, state, active_proxy })
    }

    /// SQLite 事务已由 `BEGIN IMMEDIATE` 串行化；读取上一份授权形成完整重入 fence。
    pub(crate) async fn proxy_usage_context_inner(
        &mut self,
        lot_id: LotId,
        user_id: UserId,
    ) -> Result<ProxyUsageContext, CompetitionTransactionError> {
        let rows = {
            let connection = self.connection_mut()?;
            sqlx::query(
                "SELECT authorization_id, lot_id, user_id, creation_command_id,
                        authorization_sequence, usage_number, maximum_amount, mode, status,
                        terminal_reason, state_version
                 FROM bid_proxy_authorizations
                 WHERE lot_id = ? AND user_id = ?
                 ORDER BY usage_number, authorization_sequence, authorization_id",
            )
            .bind(uuid_blob(lot_id.as_uuid()))
            .bind(uuid_blob(user_id.as_uuid()))
            .fetch_all(&mut *connection)
            .await
            .map_err(map_sql_error)?
        };
        let mut history = Vec::with_capacity(rows.len());
        for row in rows {
            let terminal_reason =
                row.try_get::<Option<String>, _>("terminal_reason").map_err(map_sql_error)?;
            let authorization = proxy_from_row(row)?;
            if !valid_proxy_terminal_reason(authorization.status, terminal_reason.as_deref()) {
                return Err(invalid_state("代理授权终态原因无效"));
            }
            history.push(authorization);
        }
        ProxyUsageContext::from_owner_history(lot_id, user_id, &history)
    }

    /// `BEGIN IMMEDIATE` 已固定写入视图；读取完整历史以生成旧版代理命令结果。
    pub(crate) async fn proxy_transition_history_inner(
        &mut self,
        authorization_id: ProxyAuthorizationId,
    ) -> Result<Vec<CompetitionProxyTransitionRecord>, CompetitionTransactionError> {
        if authorization_id.is_nil() {
            return Err(invalid_state("代理转换历史缺少授权标识"));
        }
        let rows = sqlx::query(
            "SELECT transition_number, transition_kind, old_maximum_amount,
                    new_maximum_amount, old_mode, new_mode, old_status, new_status,
                    reason, resulting_state_version, created_at
             FROM bid_proxy_authorization_transitions
             WHERE authorization_id = ?
             ORDER BY transition_number",
        )
        .bind(uuid_blob(authorization_id.as_uuid()))
        .fetch_all(&mut *self.connection_mut()?)
        .await
        .map_err(map_sql_error)?;

        rows.into_iter()
            .map(|row| {
                let created_at: String = row.try_get("created_at").map_err(map_sql_error)?;
                Ok(CompetitionProxyTransitionRecord {
                    transition_number: row.try_get("transition_number").map_err(map_sql_error)?,
                    kind: row.try_get("transition_kind").map_err(map_sql_error)?,
                    old_maximum_amount: row.try_get("old_maximum_amount").map_err(map_sql_error)?,
                    new_maximum_amount: row.try_get("new_maximum_amount").map_err(map_sql_error)?,
                    old_mode: row.try_get("old_mode").map_err(map_sql_error)?,
                    new_mode: row.try_get("new_mode").map_err(map_sql_error)?,
                    old_status: row.try_get("old_status").map_err(map_sql_error)?,
                    new_status: row.try_get("new_status").map_err(map_sql_error)?,
                    reason: row.try_get("reason").map_err(map_sql_error)?,
                    resulting_state_version: row
                        .try_get("resulting_state_version")
                        .map_err(map_sql_error)?,
                    created_at: parse_timestamp(created_at, "SQLite 代理转换时间")?,
                })
            })
            .collect()
    }

    /// 历史新版账本重放只读取冻结策略，不重新套用当前 lifecycle/时间窗 gate。
    pub(crate) async fn proxy_quota_limit_inner(
        &mut self,
        lot_id: LotId,
    ) -> Result<u32, CompetitionTransactionError> {
        let value = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(
                    CAST(json_extract(a.policy_snapshot, '$.proxy_max_outbid_uses') AS INTEGER),
                    0
                )
             FROM lots AS l
             INNER JOIN auctions AS a ON a.id = l.auction_id
             WHERE l.id = ?",
        )
        .bind(uuid_blob(lot_id.as_uuid()))
        .fetch_optional(&mut *self.connection_mut()?)
        .await
        .map_err(map_sql_error)?
        .ok_or(CompetitionTransactionError::NotFound)?;
        u32::try_from(value).map_err(|_| invalid_state("Lite 代理配额超出兼容范围"))
    }
}
