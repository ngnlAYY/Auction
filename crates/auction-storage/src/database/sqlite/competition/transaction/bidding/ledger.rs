//! SQLite 竞价事务的命令账本读写。

use sqlx::Row;

use super::super::mapping::*;
use super::super::*;

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
}
