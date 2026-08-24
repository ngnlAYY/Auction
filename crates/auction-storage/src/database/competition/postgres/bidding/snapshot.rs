//! PostgreSQL 竞价事务的锁内 lot 快照与代理使用次数读取。

use sqlx::Row;

use super::super::mapping::*;
use super::super::*;

impl PostgresCompetitionTransaction {
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
}
