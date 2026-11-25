//! 资金命令账本的重放与冲突判定：同命令 ID 同请求返回原结果，同 ID 异请求冲突（不变量 1）。

use auction_application::settlement::{FundingError, FundingMutation};
use sqlx::{Postgres, Row};
use uuid::Uuid;

use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement::funding::storage;

/// 只登记命令账本，不改账户版本与任何分配：状态迁移已在目标状态（无变化）时使用。
pub(super) async fn record_command(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    command: &FundingMutation,
    action: &str,
    target: Uuid,
) -> Result<(), FundingError> {
    sqlx::query(
        r#"INSERT INTO public.settlement_funding_commands
               (command_id, uid, action, target_id, request_digest, result)
           VALUES ($1, $2, $3, $4, $5, '{}')"#,
    )
    .bind(command.command_id)
    .bind(uid)
    .bind(action)
    .bind(target)
    .bind(command.request_digest.as_slice())
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    Ok(())
}

/// 命令已提交且请求一致返回 `true`；同 ID 但账户、动作、目标或摘要不同为冲突。
/// 与拍卖的 `ledger::replay` 不同，这里不要求账户 ready：冻结账户仍须能记录已发生的外发结果。
pub(super) async fn replay_command(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    command: &FundingMutation,
    action: &str,
    target: Uuid,
) -> Result<bool, FundingError> {
    if command.command_id.is_nil() || command.request_digest == [0; 32] {
        return Err(FundingError::Invalid);
    }
    let row = sqlx::query(
        r#"SELECT uid, action, target_id, request_digest
           FROM public.settlement_funding_commands
           WHERE command_id=$1"#,
    )
    .bind(command.command_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?;
    let Some(row) = row else { return Ok(false) };
    if row.try_get::<String, _>("uid").map_err(storage)? != uid
        || row.try_get::<String, _>("action").map_err(storage)? != action
        || row.try_get::<Uuid, _>("target_id").map_err(storage)? != target
        || row.try_get::<Vec<u8>, _>("request_digest").map_err(storage)? != command.request_digest
    {
        return Err(FundingError::Conflict);
    }
    Ok(true)
}
