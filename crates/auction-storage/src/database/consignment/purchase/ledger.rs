//! 购买与 v2 改价的账本仲裁与追加：同命令 ID 同请求回放原回执，异请求冲突，确定拒绝只占该次命令 ID。
//!
//! 保护不变量 1、3、6、8：回执、类型化目标与审计同事务追加；`command_id_conflict` 不入账。
//! 购买与 v2 改价不在 `DepositOperation` 闭集内，共享的 `DepositTarget` 与写入器是集成人串行点，
//! 所以本模块直接对 `consignment_commands` 做同形的仲裁与追加，列与 CHECK 由 000136 定义。

use auction_application::consignment::ConsignmentError;
use auction_application::core::RequestHash;
use auction_time::Timestamp;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::database::consignment::commands::errors::WriteError;
use crate::database::operations::audit_health::AuditWriteResult as _;

/// 账本操作码；与 000136 的 `operation` CHECK 逐项对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Operation {
    Purchase,
    Reprice,
}

impl Operation {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Purchase => "purchase",
            Self::Reprice => "reprice",
        }
    }
}

/// 一条待仲裁命令的身份：玩家演员、目标挂单与规范摘要。
pub(super) struct CommandIdentity<'a> {
    pub(super) command_id: Uuid,
    /// 买家（购买）或卖家（改价）的本站 UUID。
    pub(super) user_id: Uuid,
    pub(super) operation: Operation,
    pub(super) listing_id: Uuid,
    pub(super) digest: RequestHash,
    /// 仅进审计，不进摘要。
    pub(super) request_id: Option<&'a str>,
}

/// 已存回执：状态码、稳定码与 JSON 正文。
pub(super) struct StoredReceipt {
    pub(super) status: i16,
    pub(super) code: String,
    pub(super) body: Value,
}

/// 仲裁结果：已有回执原样回放，或本事务首次写入。
pub(super) enum Arbitration {
    Replay(StoredReceipt),
    First,
}

/// 不变量 1：先核演员、操作、目标与摘要，再由调用方判开关、资格与版本。
/// 同 ID 异请求（含跨演员、跨操作）一律冲突且不写账本。
pub(super) async fn arbitrate(
    connection: &mut PgConnection,
    command: &CommandIdentity<'_>,
) -> Result<Arbitration, WriteError> {
    let row = sqlx::query(
        "SELECT schema_version, actor_kind, actor_ref, operation, request_target, request_digest,
                target_kind, target_id, result_status, result_code, result_body
         FROM public.consignment_commands WHERE command_id = $1",
    )
    .bind(command.command_id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(row) = row else {
        return Ok(Arbitration::First);
    };
    let digest: &[u8] = row.try_get("request_digest")?;
    let same_identity = row.try_get::<i16, _>("schema_version")? == 2
        && row.try_get::<&str, _>("actor_kind")? == "public_user"
        && row.try_get::<&str, _>("actor_ref")? == command.user_id.to_string()
        && row.try_get::<&str, _>("operation")? == command.operation.as_str()
        && row.try_get::<&str, _>("request_target")? == command.listing_id.to_string()
        && row.try_get::<Option<&str>, _>("target_kind")? == Some("listing")
        && row.try_get::<Option<Uuid>, _>("target_id")? == Some(command.listing_id)
        && digest.len() == 32
        && bool::from(digest.ct_eq(command.digest.as_bytes().as_slice()));
    if !same_identity {
        return Err(ConsignmentError::CommandIdConflict.into());
    }
    Ok(Arbitration::Replay(StoredReceipt {
        status: row.try_get("result_status")?,
        code: row.try_get("result_code")?,
        body: row.try_get("result_body")?,
    }))
}

/// 追加回执行与审计。成功回执的正文须满足 `c4_valid_v2_receipt`；拒绝只给安全码。
pub(super) async fn insert_receipt(
    connection: &mut PgConnection,
    command: &CommandIdentity<'_>,
    receipt: &StoredReceipt,
    now: Timestamp,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ($1, 2, 'public_user', $2, $3, $4, $5, 'listing', $6, $6, $7, $8, $9, $10)",
    )
    .bind(command.command_id)
    .bind(command.user_id.to_string())
    .bind(command.operation.as_str())
    .bind(command.listing_id.to_string())
    .bind(command.digest.as_bytes().as_slice())
    .bind(command.listing_id)
    .bind(receipt.status)
    .bind(&receipt.code)
    .bind(&receipt.body)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    // 审计只记命令与处置码，不含装备、价格以外的业务内容或任何凭据。
    let disposition = receipt.body.get("disposition").and_then(Value::as_str).unwrap_or("unknown");
    sqlx::query(
        "INSERT INTO public.audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id, result,
             request_id, source, metadata)
         VALUES ($1, $2, 'public_user', $3, $4, 'consignment_command', $5, $6, $7, 'public', $8)",
    )
    .bind(Uuid::new_v4())
    .bind(now)
    .bind(command.user_id.to_string())
    .bind(format!("consignment.{}", command.operation.as_str()))
    .bind(command.command_id.to_string())
    .bind(if receipt.status < 400 { "success" } else { "rejected" })
    .bind(command.request_id)
    .bind(json!({
        "command_id": command.command_id,
        "code": receipt.code,
        "disposition": disposition,
    }))
    .execute(&mut *connection)
    .await
    .record_audit_failure()?;
    Ok(())
}

/// 确定拒绝的安全回执：只含固定头与稳定码。状态码无法确定地入账时失败关闭，不回退成别的状态。
pub(super) fn rejection_receipt(
    command: &CommandIdentity<'_>,
    status: u16,
    code: &str,
) -> Result<StoredReceipt, WriteError> {
    let Ok(status) = i16::try_from(status) else {
        return Err(WriteError::invariant("consignment_rejection_status"));
    };
    Ok(StoredReceipt {
        status,
        code: code.to_owned(),
        body: json!({
            "schema_version": 2,
            "operation": command.operation.as_str(),
            "disposition": "rejected",
            "command_id": command.command_id.to_string(),
            "code": code,
        }),
    })
}

#[cfg(test)]
mod tests {
    use auction_application::core::RequestHash;
    use uuid::Uuid;

    use super::{CommandIdentity, Operation, rejection_receipt};

    fn identity() -> CommandIdentity<'static> {
        CommandIdentity {
            command_id: Uuid::nil(),
            user_id: Uuid::nil(),
            operation: Operation::Purchase,
            listing_id: Uuid::nil(),
            digest: RequestHash::new([0; 32]),
            request_id: None,
        }
    }

    /// 能确定入账的状态码原样写入；超出 i16 的状态码失败关闭，不回退成 500。
    #[test]
    fn rejection_receipt_fails_closed_on_an_unrepresentable_status() {
        let stored = rejection_receipt(&identity(), 503, "funding_unavailable").expect("503");
        assert_eq!((stored.status, stored.code.as_str()), (503, "funding_unavailable"));
        assert!(rejection_receipt(&identity(), 40_000, "funding_unavailable").is_err());
    }
}
