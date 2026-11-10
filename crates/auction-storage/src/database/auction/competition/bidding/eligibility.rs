//! PostgreSQL 竞价事务的参与用户资格锁。

use super::super::mapping::*;
use crate::database::auction::competition::PostgresCompetitionTransaction;
use auction_application::core::CompetitionTransactionError;
use auction_domain::UserId;

impl PostgresCompetitionTransaction {
    /// lot 与活动代理已经由调用方锁定；这里按调用方给出的稳定用户顺序锁定 users owner 事实。
    pub(crate) async fn participant_is_eligible_for_update_inner(
        &mut self,
        user_id: UserId,
    ) -> Result<bool, CompetitionTransactionError> {
        let status = {
            let transaction = self.transaction_mut()?;
            sqlx::query_scalar::<_, String>(
                "SELECT status FROM public.users WHERE id = $1 FOR UPDATE",
            )
            .bind(user_id.as_uuid())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(map_sql_error)?
        };
        let Some(status) = status else {
            return Ok(false);
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
