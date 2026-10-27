//! 旧版已完成结算账本的 PostgreSQL 只读快照核验。

use auction_application::{
    ApplicationError, LegacySettlementVerification, RepositoryError, SettleAuctionCommand,
    settlement_complete_request_hash, verify_legacy_settlement_replay,
};
use auction_domain::{AuctionId, CommandId};
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use super::super::{PostgresCompetitionTransaction, PostgresStore};

/// 在 PostgreSQL 可证明一致的只读快照中核验旧 complete 账本；该路径没有任何写语句。
pub(super) async fn verify_postgres(
    store: &PostgresStore,
    auction_uuid: Uuid,
) -> Result<LegacySettlementVerification, ApplicationError> {
    if auction_uuid.is_nil() {
        return Err(storage_error("旧版结算核验拍卖标识无效"));
    }

    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开始旧版结算只读核验事务失败"))?;
    // DEFERRABLE 让 PostgreSQL 等待可证明安全的 Serializable 快照，避免只读核验因并发写入
    // 读到不能串行化的组合；该语句必须是事务中的第一条命令。
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE READ ONLY DEFERRABLE")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("建立旧版结算只读快照失败"))?;
    let mut transaction = PostgresCompetitionTransaction::new(transaction);

    let identity = sqlx::query(
        r#"SELECT settlement_workflow_id, settlement_claim_command_id,
                  settlement_complete_command_id, settlement_due_at
           FROM public.auctions
           WHERE id = $1 AND status = 'settled'"#,
    )
    .bind(auction_uuid)
    .fetch_optional(
        &mut **transaction
            .transaction_mut()
            .map_err(|_| storage_error("旧版结算只读核验事务已经结束"))?,
    )
    .await
    .map_err(|_| storage_error("读取旧版结算 workflow 身份失败"))?
    .ok_or_else(|| storage_error("旧版结算核验目标不存在或尚未 settled"))?;

    let workflow_id =
        required_uuid(&identity, "settlement_workflow_id", "旧版结算 workflow 标识缺失")?;
    let claim_command_uuid =
        required_uuid(&identity, "settlement_claim_command_id", "旧版结算 claim 命令标识缺失")?;
    let complete_command_uuid = required_uuid(
        &identity,
        "settlement_complete_command_id",
        "旧版结算 complete 命令标识缺失",
    )?;
    let due_at = identity
        .try_get::<Option<DateTime<Utc>>, _>("settlement_due_at")
        .map_err(|_| storage_error("读取旧版结算 due 时间失败"))?
        .ok_or_else(|| storage_error("旧版结算 due 时间缺失"))?;

    let stored = transaction
        .find_command_read_only_inner(CommandId::from_uuid(complete_command_uuid))
        .await
        .map_err(|_| storage_error("读取旧版结算 complete 账本失败"))?
        .ok_or_else(|| storage_error("旧版结算 complete 账本缺失"))?;
    if stored.command_kind != "settlement_complete" {
        return Err(storage_error("目标命令不是旧版 settlement_complete 账本"));
    }
    let auction_id = AuctionId::from_uuid(auction_uuid);
    let request_hash = settlement_complete_request_hash(auction_uuid, workflow_id, due_at)
        .map_err(|_| storage_error("生成当前结算请求摘要失败"))?;
    let snapshot = transaction
        .load_settled_snapshot_read_only(auction_id)
        .await
        .map_err(ApplicationError::from)?;
    let verification = verify_legacy_settlement_replay(
        SettleAuctionCommand {
            command_id: CommandId::from_uuid(complete_command_uuid),
            workflow_id,
            claim_command_id: CommandId::from_uuid(claim_command_uuid),
            auction_id,
            due_at,
            // 旧 Worker 将冻结的 due 世代同时作为 complete 命令的 Now。
            now: due_at,
            request_hash,
        },
        stored,
        snapshot,
    )?;
    transaction.commit().await.map_err(|_| storage_error("提交旧版结算只读核验事务失败"))?;
    Ok(verification)
}

fn required_uuid(
    row: &sqlx::postgres::PgRow,
    column: &str,
    missing: &str,
) -> Result<Uuid, ApplicationError> {
    let value = row
        .try_get::<Option<Uuid>, _>(column)
        .map_err(|_| storage_error("读取旧版结算命令身份失败"))?
        .ok_or_else(|| storage_error(missing))?;
    if value.is_nil() {
        return Err(storage_error(missing));
    }
    Ok(value)
}

fn storage_error(message: &str) -> ApplicationError {
    RepositoryError::Failed(message.to_owned()).into()
}
