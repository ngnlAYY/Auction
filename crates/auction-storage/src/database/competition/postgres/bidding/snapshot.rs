//! PostgreSQL 竞价事务的锁内 lot 快照与代理使用次数读取。

use sqlx::Row;

use super::super::mapping::*;
use super::super::*;

impl PostgresCompetitionTransaction {
    pub(crate) async fn current_time_inner(
        &mut self,
    ) -> Result<DateTime<Utc>, CompetitionTransactionError> {
        sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut **self.transaction_mut()?)
            .await
            .map_err(map_sql_error)
    }

    pub(crate) async fn load_for_update_inner(
        &mut self,
        lot_id: LotId,
        now: DateTime<Utc>,
    ) -> Result<CompetitionSnapshot, CompetitionTransactionError> {
        self.load_for_update_with_deadline_gate(lot_id, now, true).await
    }

    /// 生命周期 Final 转换复用竞价快照形状，但允许读取已经到期的 live lot；公开竞价仍必须
    /// 走带截止门禁的 `load_for_update_inner`。
    pub(crate) async fn load_for_final_transition_inner(
        &mut self,
        lot_id: LotId,
        now: DateTime<Utc>,
    ) -> Result<CompetitionSnapshot, CompetitionTransactionError> {
        self.load_for_update_with_deadline_gate(lot_id, now, false).await
    }

    async fn load_for_update_with_deadline_gate(
        &mut self,
        lot_id: LotId,
        now: DateTime<Utc>,
        require_unexpired: bool,
    ) -> Result<CompetitionSnapshot, CompetitionTransactionError> {
        let row = {
            let transaction = self.transaction_mut()?;
            sqlx::query(
                r#"WITH target_lot AS MATERIALIZED (
                       SELECT lot.auction_id, auction.world_id
                       FROM public.lots AS lot
                       INNER JOIN public.auctions AS auction ON auction.id = lot.auction_id
                       WHERE lot.id = $1
                   ),
                   locked_world AS MATERIALIZED (
                       SELECT world.id, world.enabled
                       FROM public.worlds AS world
                       INNER JOIN target_lot ON target_lot.world_id = world.id
                       FOR UPDATE OF world
                   ),
                   locked_auction AS MATERIALIZED (
                       SELECT
                           auction.id, auction.world_id, auction.status, auction.scheduled_open_at,
                           auction.default_start_price, auction.soft_close_window_seconds,
                           auction.soft_close_extension_seconds, auction.cancel_cutoff_seconds,
                           auction.proxy_slow_interval_seconds, auction.policy_snapshot
                       FROM public.auctions AS auction
                       INNER JOIN target_lot ON target_lot.auction_id = auction.id
                       INNER JOIN locked_world ON locked_world.id = auction.world_id
                       FOR UPDATE OF auction
                   )
                   SELECT
                    l.start_price, l.current_price, l.leader_user_id, l.leader_proxy_last,
                    l.effective_end_at, l.state_version, l.last_proxy_sequence,
                    l.slow_generation, l.slow_authorization_id, l.slow_due_at, l.material,
                    l.status AS lot_status,
                    locked_auction.status AS auction_status,
                    locked_auction.scheduled_open_at,
                    locked_world.enabled AS world_enabled,
                    locked_auction.default_start_price,
                    locked_auction.soft_close_window_seconds,
                    locked_auction.soft_close_extension_seconds,
                    locked_auction.cancel_cutoff_seconds,
                    locked_auction.proxy_slow_interval_seconds,
                    COALESCE((locked_auction.policy_snapshot ->> 'bid_increment_ppm')::bigint, 0)
                        AS policy_bid_increment_ppm,
                    COALESCE((locked_auction.policy_snapshot ->> 'minimum_increment')::bigint, 30000)
                        AS policy_minimum_increment,
                    COALESCE((locked_auction.policy_snapshot ->> 'proxy_enabled')::boolean, true)
                        AS policy_proxy_enabled,
                    COALESCE(locked_auction.policy_snapshot ->> 'proxy_default_mode', 'fast')
                        AS policy_proxy_default_mode,
                    COALESCE((locked_auction.policy_snapshot ->> 'proxy_max_outbid_uses')::bigint, 0)
                        AS policy_proxy_max_outbid_uses,
                    COALESCE((locked_auction.policy_snapshot ->> 'final_phase_lead_seconds')::bigint, 900)
                        AS policy_final_phase_lead_seconds
                 FROM public.lots AS l
                 INNER JOIN locked_auction ON locked_auction.id = l.auction_id
                 INNER JOIN locked_world ON locked_world.id = locked_auction.world_id
                 WHERE l.id = $1
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

        let world_enabled: bool = row.try_get("world_enabled").map_err(map_sql_error)?;
        let auction_status =
            parse_auction_status(row.try_get("auction_status").map_err(map_sql_error)?)?;
        let lot_status: String = row.try_get("lot_status").map_err(map_sql_error)?;
        let scheduled_open_at: DateTime<Utc> =
            row.try_get("scheduled_open_at").map_err(map_sql_error)?;
        let effective_end_at =
            row.try_get::<Option<DateTime<Utc>>, _>("effective_end_at").map_err(map_sql_error)?;
        if !world_enabled
            || !matches!(auction_status, AuctionStatus::Live | AuctionStatus::Final)
            || lot_status != "live"
            || now < scheduled_open_at
        {
            return Err(CompetitionTransactionError::InvalidState);
        }
        let effective_end_at = effective_end_at.ok_or(CompetitionTransactionError::InvalidState)?;
        if require_unexpired && now >= effective_end_at {
            return Err(CompetitionTransactionError::InvalidState);
        }

        self.snapshot_from_row(lot_id, row, effective_end_at, auction_status).await
    }

    /// 结算已经先锁定 auction、pool 和全部 lot；这里只读取竞价规则并锁定活动代理。
    ///
    /// 该入口只接受 `settling`，不能复用公开竞价的 `live`/`final` 时间窗查询，否则会把
    /// 结算状态错误地开放给浏览器竞价，或再次阻断正常结算。
    pub(crate) async fn load_for_settlement_inner(
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
                    a.status AS auction_status,
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
                    COALESCE((a.policy_snapshot ->> 'final_phase_lead_seconds')::bigint, 900)
                        AS policy_final_phase_lead_seconds
                 FROM public.lots AS l
                 INNER JOIN public.auctions AS a ON a.id = l.auction_id
                 WHERE l.id = $1 AND l.status = 'live' AND a.status = 'settling'
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
        let effective_end_at = row
            .try_get::<Option<DateTime<Utc>>, _>("effective_end_at")
            .map_err(map_sql_error)?
            .ok_or_else(|| invalid_state("结算 lot 缺少有效结束时间"))?;
        let auction_status =
            parse_auction_status(row.try_get("auction_status").map_err(map_sql_error)?)?;
        self.snapshot_from_row(lot_id, row, effective_end_at, auction_status).await
    }

    async fn snapshot_from_row(
        &mut self,
        lot_id: LotId,
        row: PgRow,
        effective_end_at: DateTime<Utc>,
        auction_status: AuctionStatus,
    ) -> Result<CompetitionSnapshot, CompetitionTransactionError> {
        let rules = auction_rules_from_row(&row)?;
        let stored_start_price: i64 = row.try_get("start_price").map_err(map_sql_error)?;
        let start_price = BidAmount::from_persisted_positive(stored_start_price)
            .map_err(|_| invalid_state("lot 起始价不是正数"))?;
        let stored_current_price: Option<i64> =
            row.try_get("current_price").map_err(map_sql_error)?;
        let leader_id = row
            .try_get::<Option<Uuid>, _>("leader_user_id")
            .map_err(map_sql_error)?
            .map(auction_domain::UserId::from_uuid);
        // Auction-BF v28 与缺陷版本的 Open 都可能用“起始价 + 无领先者”作持久化哨兵；
        // 领域层用 None 表示同一事实，只兼容这一种精确形状，避免吞掉真实损坏状态。
        let current_price = match (stored_current_price, leader_id.is_some()) {
            (None, false) => None,
            (Some(current_price), false) if current_price == stored_start_price => None,
            (Some(current_price), true) => Some(
                BidAmount::from_persisted_positive(current_price)
                    .map_err(|_| invalid_state("lot 当前价不是正数"))?,
            ),
            _ => return Err(invalid_state("lot 当前价与领先者持久化投影不一致")),
        };
        let state = LotBidState {
            lot_id,
            start_price,
            current_price,
            leader_id,
            leader_proxy_last: row.try_get("leader_proxy_last").map_err(map_sql_error)?,
            material: row.try_get("material").map_err(map_sql_error)?,
            effective_end_at,
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
        Ok(CompetitionSnapshot { rules, auction_status, state, active_proxy })
    }

    /// lot 已由调用方锁定；读取上一份授权，使重入编号和最低上限使用同一事务事实。
    pub(crate) async fn proxy_usage_context_inner(
        &mut self,
        lot_id: LotId,
        user_id: UserId,
    ) -> Result<ProxyUsageContext, CompetitionTransactionError> {
        let rows = {
            let transaction = self.transaction_mut()?;
            sqlx::query(
                "SELECT authorization_id, lot_id, user_id, creation_command_id,
                        authorization_sequence, usage_number, maximum_amount, mode, status,
                        terminal_reason, state_version
                 FROM public.bid_proxy_authorizations
                 WHERE lot_id = $1 AND user_id = $2
                 ORDER BY usage_number, authorization_sequence, authorization_id
                 FOR UPDATE",
            )
            .bind(lot_id.as_uuid())
            .bind(user_id.as_uuid())
            .fetch_all(&mut **transaction)
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

    /// 活动授权行已由快照读取锁定；这里读取完整 append-only 历史以生成旧版命令结果。
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
             FROM public.bid_proxy_authorization_transitions
             WHERE authorization_id = $1
             ORDER BY transition_number",
        )
        .bind(authorization_id.as_uuid())
        .fetch_all(&mut **self.transaction_mut()?)
        .await
        .map_err(map_sql_error)?;

        rows.into_iter()
            .map(|row| {
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
                    created_at: row.try_get("created_at").map_err(map_sql_error)?,
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
            "SELECT COALESCE((a.policy_snapshot ->> 'proxy_max_outbid_uses')::bigint, 0)
             FROM public.lots AS l
             INNER JOIN public.auctions AS a ON a.id = l.auction_id
             WHERE l.id = $1",
        )
        .bind(lot_id.as_uuid())
        .fetch_optional(&mut **self.transaction_mut()?)
        .await
        .map_err(map_sql_error)?
        .ok_or(CompetitionTransactionError::NotFound)?;
        u32::try_from(value).map_err(|_| invalid_state("代理配额超出兼容范围"))
    }
}
