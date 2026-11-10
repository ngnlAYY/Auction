//! PostgreSQL 竞价事务的命令账本读写。
//!
//! 不变量：同一 `command_id` 同请求返回原结果，不同请求冲突。命令身份仍由唯一主键
//! 仲裁，读路径只做非锁定投影：对不存在的命令行做 `SELECT … FOR UPDATE` 会在
//! Serializable 下取得写侧页谓词锁，紧随其后的 `INSERT … ON CONFLICT` 再写同一索引页，
//! 使逻辑无关的两个竞价事务互相中止（跨 lot 40001）。因此：
//! - 未命中只来自普通读取，交给 INSERT 仲裁；
//! - 只有 INSERT 真正冲突（并发同 ID 已提交）时才用锁定读取取最新版本，并要求调用方
//!   按账本结果重放，不能再次应用计划。

use sqlx::Row;

use super::super::mapping::*;
use crate::database::auction::competition::PostgresCompetitionTransaction;
use auction_application::core::{
    CommandInsert, CommandLedgerError, CompetitionTransactionError, RequestHash, StoredCommand,
};
use serde_json::Value;

impl PostgresCompetitionTransaction {
    /// 非锁定读取：命中即为已提交账本事实，可直接重放。
    pub(crate) async fn find_command_inner(
        &mut self,
        command_id: auction_domain::CommandId,
    ) -> Result<Option<StoredCommand>, CompetitionTransactionError> {
        let row = {
            let transaction = self.transaction_mut()?;
            sqlx::query(
                r#"SELECT command_kind, aggregate_id, request_hash, result_code, result_payload
                   FROM public.auction_commands
                   WHERE command_id = $1"#,
            )
            .bind(command_id.as_uuid())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(map_sql_error)?
        };
        self.stored_command_from_row(command_id, row)
    }

    /// 冲突路径专用：锁定读取会看到并发事务刚提交的版本，普通读取看不到。
    async fn find_command_for_update_inner(
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
        self.stored_command_from_row(command_id, row)
    }

    fn stored_command_from_row(
        &self,
        command_id: auction_domain::CommandId,
        row: Option<sqlx::postgres::PgRow>,
    ) -> Result<Option<StoredCommand>, CompetitionTransactionError> {
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
        command: &StoredCommand,
    ) -> Result<CommandInsert, CompetitionTransactionError> {
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
            return Ok(CommandInsert::Inserted);
        }

        // 并发同 ID 已提交：只有这里需要锁定读取才能看到新版本；调用方按结果重放，
        // 不再应用本事务计划，避免同一命令写两遍事实。
        let Some(existing) = self.find_command_for_update_inner(command.command_id).await? else {
            return Err(CompetitionTransactionError::Failed(
                "命令冲突后无法读取已有账本记录".to_owned(),
            ));
        };
        if existing == *command {
            Ok(CommandInsert::Duplicate(existing))
        } else {
            Err(CompetitionTransactionError::CommandLedger(CommandLedgerError::Conflict))
        }
    }
}
