//! SQLite 竞价事务的锁内 lot 快照与代理使用次数读取。

use sqlx::Row;

use super::super::mapping::*;
use super::super::*;

impl SqliteCompetitionTransaction {
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
}
