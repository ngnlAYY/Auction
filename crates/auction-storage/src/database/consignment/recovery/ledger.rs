//! 恢复处置与处罚、权利通知等系统操作的账本仲裁：同命令 ID 同请求回放原回执，异请求冲突；确定拒绝不占命令 ID。
//!
//! 保护不变量 1、3、6、8：回执、类型化目标与审计同事务追加；`command_id_conflict` 不入账。
//! 三个操作不在 `DepositOperation` 闭集内，所以本模块直接对 `consignment_commands` 做同形的仲裁与追加，
//! 列与 CHECK 由 000137 的 7A-2 节定义（与 `returns::ledger` 同一写法）。

use auction_application::consignment::{
    ConsignmentError, DepositActor, DepositBusinessRejection, RecoveryOperation,
};
use auction_application::core::RequestHash;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::database::consignment::commands::errors::WriteError;
use crate::database::operations::audit_health::AuditWriteResult as _;

/// 账本目标：处置按 intake，自动重列按挂单，提醒按合同。
#[derive(Debug, Clone, Copy)]
pub(super) enum Target {
    /// `return_intake`：`resolve_return`。
    Intake(Uuid),
    /// `listing`：`auto_relist`，同时写 `listing_id`。
    Listing(Uuid),
    /// `contract`：`remind`。
    Contract(Uuid),
    /// `sanction`：`apply_sanction`。
    Sanction(Uuid),
    /// `buyer_cycle`：`reset_cycle` 的目标是周期被重置的买家，`open_sanction` 的目标是被处罚的周期。
    BuyerCycle(Uuid),
    /// `notice_work`：`deliver_right_notice`，目标是受限权利通知工作。
    NoticeWork(Uuid),
}

impl Target {
    const fn kind(self) -> &'static str {
        match self {
            Self::Intake(_) => "return_intake",
            Self::Listing(_) => "listing",
            Self::Contract(_) => "contract",
            Self::Sanction(_) => "sanction",
            Self::BuyerCycle(_) => "buyer_cycle",
            Self::NoticeWork(_) => "notice_work",
        }
    }

    const fn id(self) -> Uuid {
        match self {
            Self::Intake(id)
            | Self::Listing(id)
            | Self::Contract(id)
            | Self::Sanction(id)
            | Self::BuyerCycle(id)
            | Self::NoticeWork(id) => id,
        }
    }
}

/// 一条待仲裁的命令身份；三个操作都是 Worker 的系统命令。
#[derive(Debug, Clone, Copy)]
pub(super) struct LedgerCommand<'a> {
    pub(super) command_id: Uuid,
    pub(super) operation: RecoveryOperation,
    pub(super) target: Target,
    pub(super) digest: &'a RequestHash,
}

/// 确定拒绝：只在提交后报告给调用方，不写账本、不占命令 ID（事实补齐后同一业务键仍可重试）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Rejection {
    /// 来源与请求不一致。
    SourceMismatch,
    /// 证据互相矛盾或事实无法成立。
    EvidenceConflict,
    /// 同一业务对象已有另一份处置。
    DispositionConflict,
}

impl Rejection {
    /// 提交之后给调用方的应用错误。
    pub(super) const fn into_error(self) -> ConsignmentError {
        ConsignmentError::Deposit(match self {
            Self::SourceMismatch => DepositBusinessRejection::SourceMismatch,
            Self::EvidenceConflict => DepositBusinessRejection::EvidenceConflict,
            Self::DispositionConflict => DepositBusinessRejection::DispositionConflict,
        })
    }
}

/// 事务内一步的结果：完成，或已把确定拒绝写入账本、待提交后报告。
pub(super) enum Step<T> {
    /// 业务事实与成功回执已写入。
    Done(T),
    /// 确定拒绝，待提交后报告。
    Rejected(Rejection),
}

impl<T> Step<T> {
    /// 提交之后把账本里的确定拒绝还原成应用错误。
    pub(super) fn into_result(self) -> Result<T, ConsignmentError> {
        match self {
            Self::Done(value) => Ok(value),
            Self::Rejected(rejection) => Err(rejection.into_error()),
        }
    }
}

/// 仲裁结果：已有成功回执原样回放，或本事务首次写入。
pub(super) enum Arbitration {
    /// 同 ID 同请求的原成功回执正文。
    Replay(Value),
    /// 首次出现的命令 ID。
    First,
}

/// 不变量 1：先核身份与摘要。同 ID 异请求冲突且不写账本；已存确定拒绝按原样重放为错误。
pub(super) async fn arbitrate(
    connection: &mut PgConnection,
    command: &LedgerCommand<'_>,
) -> Result<Step<Arbitration>, WriteError> {
    let row = sqlx::query(
        "SELECT schema_version, actor_kind, actor_ref, operation, request_target, request_digest,
                target_kind, result_status, result_code, result_body
         FROM public.consignment_commands WHERE command_id = $1",
    )
    .bind(command.command_id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(row) = row else {
        return Ok(Step::Done(Arbitration::First));
    };
    let digest: &[u8] = row.try_get("request_digest")?;
    let same_identity = row.try_get::<i16, _>("schema_version")? == 2
        && row.try_get::<&str, _>("actor_kind")? == command.operation.actor_kind()
        && row.try_get::<&str, _>("actor_ref")? == DepositActor::DEPOSIT_SERVICE_REF
        && row.try_get::<&str, _>("operation")? == command.operation.as_str()
        && row.try_get::<&str, _>("request_target")? == command.target.id().to_string()
        && row.try_get::<Option<&str>, _>("target_kind")? == Some(command.target.kind())
        && digest.len() == 32
        && bool::from(digest.ct_eq(command.digest.as_bytes().as_slice()));
    if !same_identity {
        return Err(ConsignmentError::CommandIdConflict.into());
    }
    // 本模块的确定拒绝不占命令 ID（事实补齐后同一业务键仍可重试），账本里只会有成功回执。
    let status: i16 = row.try_get("result_status")?;
    if status >= 400 {
        return Err(WriteError::invariant("consignment_recovery_stored_rejection"));
    }
    Ok(Step::Done(Arbitration::Replay(row.try_get("result_body")?)))
}

/// 写入成功回执；`fields` 是该操作的必有字段，公共头由本函数补齐。
pub(super) async fn insert_success(
    connection: &mut PgConnection,
    command: &LedgerCommand<'_>,
    disposition: &str,
    fields: Value,
) -> Result<(), WriteError> {
    let mut body = json!({
        "schema_version": 2,
        "operation": command.operation.as_str(),
        "disposition": disposition,
        "command_id": command.command_id.to_string(),
    });
    if let (Some(target), Value::Object(extra)) = (body.as_object_mut(), fields) {
        target.extend(extra);
    }
    insert(connection, command, 200, "ok", &body, disposition).await
}

/// 账本行与审计行同事务追加；审计只记命令与处置码，不含装备、预约码或凭据。
async fn insert(
    connection: &mut PgConnection,
    command: &LedgerCommand<'_>,
    status: i16,
    code: &str,
    body: &Value,
    disposition: &str,
) -> Result<(), WriteError> {
    let listing_id = match command.target {
        Target::Listing(id) => Some(id),
        Target::Intake(_)
        | Target::Contract(_)
        | Target::Sanction(_)
        | Target::BuyerCycle(_)
        | Target::NoticeWork(_) => None,
    };
    sqlx::query(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, target_bot_uid, target_world_id,
             target_external_mid, listing_id, result_status, result_code, result_body, created_at)
         VALUES ($1, 2, $12, $2, $3, $4, $5, $6, $7, NULL, NULL, NULL, $8, $9, $10, $11,
                 transaction_timestamp())",
    )
    .bind(command.command_id)
    .bind(DepositActor::DEPOSIT_SERVICE_REF)
    .bind(command.operation.as_str())
    .bind(command.target.id().to_string())
    .bind(command.digest.as_bytes().as_slice())
    .bind(command.target.kind())
    .bind(command.target.id())
    .bind(listing_id)
    .bind(status)
    .bind(code)
    .bind(body)
    .bind(command.operation.actor_kind())
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        "INSERT INTO public.audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id, result,
             request_id, source, metadata)
         VALUES ($1, transaction_timestamp(), $7, $2, $3, 'consignment_command', $4, $5,
                 NULL, 'worker', $6)",
    )
    .bind(Uuid::new_v4())
    .bind(DepositActor::DEPOSIT_SERVICE_REF)
    .bind(format!("consignment.{}", command.operation.as_str()))
    .bind(command.command_id.to_string())
    .bind(if status < 400 { "success" } else { "rejected" })
    .bind(json!({ "command_id": command.command_id, "disposition": disposition }))
    .bind(command.operation.actor_kind())
    .execute(&mut *connection)
    .await
    .record_audit_failure()?;
    Ok(())
}
