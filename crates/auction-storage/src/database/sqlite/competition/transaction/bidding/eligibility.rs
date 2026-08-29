//! SQLite Lite 竞价事务的参与用户资格读取。

use super::super::*;

impl SqliteCompetitionTransaction {
    /// `BEGIN IMMEDIATE` 已串行化写事务；这里在同一连接内读取 users owner 的权威资格事实。
    pub(crate) async fn participant_is_eligible_for_update_inner(
        &mut self,
        user_id: UserId,
    ) -> Result<bool, CompetitionTransactionError> {
        let status = {
            let connection = self.connection_mut()?;
            sqlx::query_scalar::<_, String>("SELECT status FROM users WHERE id = ?")
                .bind(uuid_blob(user_id.as_uuid()))
                .fetch_optional(&mut *connection)
                .await
                .map_err(map_sql_error)?
        };
        let Some(status) = status else {
            return Ok(false);
        };
        let blacklist_active = {
            let connection = self.connection_mut()?;
            sqlx::query_scalar::<_, i64>(
                "SELECT CASE WHEN revoked_at IS NULL THEN 1 ELSE 0 END
                 FROM user_blacklist
                 WHERE user_id = ?",
            )
            .bind(uuid_blob(user_id.as_uuid()))
            .fetch_optional(&mut *connection)
            .await
            .map_err(map_sql_error)?
            .unwrap_or(0)
                != 0
        };
        Ok(status == "active" && !blacklist_active)
    }
}
