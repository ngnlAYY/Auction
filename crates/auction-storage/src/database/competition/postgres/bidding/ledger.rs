//! PostgreSQL 竞价事务的命令账本读写。

use sqlx::Row;

use super::super::mapping::*;
use super::super::*;

impl PostgresCompetitionTransaction {
    pub(crate) async fn find_command_inner(
        &mut self,
        command_id: auction_domain::CommandId,
    ) -> Result<Option<StoredCommand>, CompetitionTransactionError> {
        self.find_command_with_lock(command_id, true).await
    }

    /// 在调用方已建立的一致只读快照中读取命令账本，不请求 PostgreSQL 行锁。
    pub(crate) async fn find_command_read_only_inner(
        &mut self,
        command_id: auction_domain::CommandId,
    ) -> Result<Option<StoredCommand>, CompetitionTransactionError> {
        self.find_command_with_lock(command_id, false).await
    }

    async fn find_command_with_lock(
        &mut self,
        command_id: auction_domain::CommandId,
        for_update: bool,
    ) -> Result<Option<StoredCommand>, CompetitionTransactionError> {
        let row = {
            let transaction = self.transaction_mut()?;
            let query = if for_update {
                r#"SELECT command_kind, aggregate_id, request_hash, result_code, result_payload
                   FROM public.auction_commands
                   WHERE command_id = $1
                   FOR UPDATE"#
            } else {
                r#"SELECT command_kind, aggregate_id, request_hash, result_code, result_payload
                   FROM public.auction_commands
                   WHERE command_id = $1"#
            };
            sqlx::query(query)
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
            .bind(command.request_hash.as_bytes().as_slice())
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
}
