//! 收款侧命令的回执与审计：同一事务追加 `consignment_commands` 的 v2 回执与一条审计行。
//!
//! 保护不变量 1、3、8：被接受的命令在同一提交里留下可重放的类型化回执与追加审计；回执形状照 000136
//! 的 `c4_valid_v2_receipt`（`certify_cod` 的 `payment_funded`、`register_opening_attribution` 的
//! `attribution_registered`），目标种类为 `obligation` / `receipt_key`。同命令 ID 异请求不入账：
//! 资金命令账本先以 `FundingError::Conflict` 拒绝，本表主键冲突同样映射为冲突，不写任何事实。
//! 审计只记命令、操作与处置码，不含 UID 以外的外部标识、装备 key 或凭据。

use auction_application::settlement::{FundingError, FundingMutation};
use serde_json::{Value, json};
use sqlx::Postgres;
use uuid::Uuid;

use super::receipt::funding_sql_error;
use crate::database::TrackedTransaction as Transaction;
use crate::database::operations::audit_health::AuditWriteResult as _;
use crate::database::settlement::funding::storage;

/// 账本 `actor_ref`：与派发命令共用固定服务身份。
const ACTOR_REF: &str = "consignment";

/// 一条待追加的成功回执。
pub(super) struct SuccessReceipt<'a> {
    pub(super) mutation: &'a FundingMutation,
    /// `certify_cod` 或 `register_opening_attribution`。
    pub(super) operation: &'a str,
    /// 账本目标种类：`obligation` 或 `receipt_key`。
    pub(super) target_kind: &'a str,
    pub(super) target_id: Uuid,
    /// 处置码：`payment_funded` 或 `attribution_registered`。
    pub(super) disposition: &'a str,
    /// 回执正文中除公共字段外的结果字段。
    pub(super) fields: Value,
}

/// 追加回执与审计；成功回执状态 200、结果码 `ok`。
pub(super) async fn append_success(
    tx: &mut Transaction<'_, Postgres>,
    receipt: SuccessReceipt<'_>,
) -> Result<(), FundingError> {
    let mut body = json!({
        "schema_version": 2,
        "operation": receipt.operation,
        "disposition": receipt.disposition,
        "command_id": receipt.mutation.command_id.to_string(),
    });
    if let (Some(body), Value::Object(fields)) = (body.as_object_mut(), receipt.fields) {
        body.extend(fields);
    }
    sqlx::query(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ($1, 2, 'worker', $2, $3, $4, $5, $6, $7, NULL, 200, 'ok', $8,
                 transaction_timestamp())",
    )
    .bind(receipt.mutation.command_id)
    .bind(ACTOR_REF)
    .bind(receipt.operation)
    .bind(receipt.target_id.to_string())
    .bind(receipt.mutation.request_digest.as_slice())
    .bind(receipt.target_kind)
    .bind(receipt.target_id)
    .bind(&body)
    .execute(&mut **tx)
    .await
    .map_err(funding_sql_error)?;
    sqlx::query(
        "INSERT INTO public.audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id, result,
             request_id, source, metadata)
         VALUES ($1, transaction_timestamp(), 'worker', $2, $3, 'consignment_command', $4,
                 'success', NULL, 'worker', $5)",
    )
    .bind(Uuid::new_v4())
    .bind(ACTOR_REF)
    .bind(format!("consignment.{}", receipt.operation))
    .bind(receipt.mutation.command_id.to_string())
    .bind(json!({
        "command_id": receipt.mutation.command_id,
        "disposition": receipt.disposition,
    }))
    .execute(&mut **tx)
    .await
    .record_audit_failure()
    .map_err(storage)?;
    Ok(())
}
