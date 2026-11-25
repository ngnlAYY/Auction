//! 退件接管三个操作的账本仲裁：同命令 ID 同请求回放原回执，异请求冲突，确定拒绝只占该次命令 ID。
//!
//! 保护不变量 1、3、6、8：回执、类型化目标与审计同事务追加；`command_id_conflict` 不入账。
//! 三个操作不在 `DepositOperation` 闭集内，共享的 `DepositTarget` 与写入器是集成人串行点，
//! 所以本模块直接对 `consignment_commands` 做同形的仲裁与追加，列与 CHECK 由 000136 定义。

use auction_application::consignment::{
    ConsignmentError, DepositActor, DepositBusinessRejection, ReturnOperation,
};
use auction_application::core::RequestHash;
use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::settlement_world_code;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::database::consignment::commands::errors::WriteError;
use crate::database::operations::audit_health::AuditWriteResult as _;

/// 账本目标：接管按 RTS 的 K，起写与消费按 intake。
#[derive(Debug, Clone, Copy)]
pub(super) enum Target<'a> {
    /// `mail_key`：`acquire_return`。
    MailKey(&'a OriginalMailKey),
    /// `return_intake`：`start_return_take`、`consume_return`。
    Intake(Uuid),
}

impl Target<'_> {
    const fn kind(&self) -> &'static str {
        match self {
            Self::MailKey(_) => "mail_key",
            Self::Intake(_) => "return_intake",
        }
    }

    fn request_target(&self) -> String {
        match self {
            Self::MailKey(key) => mail_key_text(key),
            Self::Intake(intake_id) => intake_id.to_string(),
        }
    }
}

fn mail_key_text(key: &OriginalMailKey) -> String {
    format!(
        "{}|{}|{}",
        key.authenticated_bot_uid(),
        settlement_world_code(key.world()),
        key.external_mid()
    )
}

/// 一条待仲裁的命令身份；接管是系统命令，起写与消费是 Worker 命令。
#[derive(Debug, Clone, Copy)]
pub(super) struct LedgerCommand<'a> {
    pub(super) command_id: Uuid,
    pub(super) operation: ReturnOperation,
    pub(super) target: Target<'a>,
    pub(super) digest: RequestHash,
}

impl LedgerCommand<'_> {
    const fn actor_kind(&self) -> &'static str {
        match self.operation {
            ReturnOperation::AcquireReturn => "system",
            ReturnOperation::StartReturnTake | ReturnOperation::ConsumeReturn => "worker",
        }
    }
}

/// 可持久化的确定拒绝；码与状态必须落在 000136 的回执白名单内。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Rejection {
    /// fence 或 claim token 已不是当前领取。
    FenceMismatch,
    /// 租约已过。
    LeaseLost,
    /// 来源、冻结身份或工作与请求不一致。
    SourceMismatch,
    /// 缺少完成动作所需的证明或前置事实。
    EvidenceUnproven,
    /// 证据或起写事实互相矛盾。
    EvidenceConflict,
    /// 当前状态不允许该动作。
    ActionNotAllowed,
    /// 相反处置或异来源。
    DispositionConflict,
}

impl Rejection {
    const ALL: [Self; 7] = [
        Self::FenceMismatch,
        Self::LeaseLost,
        Self::SourceMismatch,
        Self::EvidenceUnproven,
        Self::EvidenceConflict,
        Self::ActionNotAllowed,
        Self::DispositionConflict,
    ];

    const fn code(self) -> &'static str {
        match self {
            Self::FenceMismatch => "fence_mismatch",
            Self::LeaseLost => "lease_lost",
            Self::SourceMismatch => "source_mismatch",
            Self::EvidenceUnproven => "evidence_unproven",
            Self::EvidenceConflict => "evidence_conflict",
            Self::ActionNotAllowed => "action_not_allowed",
            Self::DispositionConflict => "disposition_conflict",
        }
    }

    const fn status(self) -> i16 {
        match self {
            Self::ActionNotAllowed => 403,
            _ => 409,
        }
    }

    /// 提交之后给调用方的应用错误；租约过期沿用 `LeaseLost`。
    pub(super) const fn into_error(self) -> ConsignmentError {
        let rejection = match self {
            Self::LeaseLost => return ConsignmentError::LeaseLost,
            Self::FenceMismatch => DepositBusinessRejection::FenceMismatch,
            Self::SourceMismatch => DepositBusinessRejection::SourceMismatch,
            Self::EvidenceUnproven => DepositBusinessRejection::EvidenceUnproven,
            Self::EvidenceConflict => DepositBusinessRejection::EvidenceConflict,
            Self::ActionNotAllowed => DepositBusinessRejection::ActionNotAllowed,
            Self::DispositionConflict => DepositBusinessRejection::DispositionConflict,
        };
        ConsignmentError::Deposit(rejection)
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|rejection| rejection.code() == code)
    }
}

/// 事务内一步的结果：完成，或已把确定拒绝写入账本、待提交后报告。
pub(super) enum Step<T> {
    /// 业务事实与成功回执已写入。
    Done(T),
    /// 确定拒绝已追加到账本。
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
        && row.try_get::<&str, _>("actor_kind")? == command.actor_kind()
        && row.try_get::<&str, _>("actor_ref")? == DepositActor::DEPOSIT_SERVICE_REF
        && row.try_get::<&str, _>("operation")? == command.operation.as_str()
        && row.try_get::<&str, _>("request_target")? == command.target.request_target()
        && row.try_get::<Option<&str>, _>("target_kind")? == Some(command.target.kind())
        && digest.len() == 32
        && bool::from(digest.ct_eq(command.digest.as_bytes().as_slice()));
    if !same_identity {
        return Err(ConsignmentError::CommandIdConflict.into());
    }
    let status: i16 = row.try_get("result_status")?;
    if status >= 400 {
        let code: String = row.try_get("result_code")?;
        let rejection = Rejection::from_code(&code)
            .ok_or_else(|| WriteError::invariant("consignment_return_rejection_code"))?;
        return Ok(Step::Rejected(rejection));
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

/// 追加确定拒绝并返回拒绝步骤；确定拒绝只占该次命令 ID，不写任何业务事实。
pub(super) async fn insert_rejection<T>(
    connection: &mut PgConnection,
    command: &LedgerCommand<'_>,
    rejection: Rejection,
) -> Result<Step<T>, WriteError> {
    let body = json!({
        "schema_version": 2,
        "operation": command.operation.as_str(),
        "disposition": "rejected",
        "command_id": command.command_id.to_string(),
        "code": rejection.code(),
    });
    insert(connection, command, rejection.status(), rejection.code(), &body, "rejected").await?;
    Ok(Step::Rejected(rejection))
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
    let (bot_uid, world_id, external_mid, target_id) = match command.target {
        Target::MailKey(key) => (
            Some(key.authenticated_bot_uid().to_owned()),
            Some(settlement_world_code(key.world())),
            Some(key.external_mid().to_owned()),
            None,
        ),
        Target::Intake(intake_id) => (None, None, None, Some(intake_id)),
    };
    sqlx::query(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, target_bot_uid, target_world_id,
             target_external_mid, listing_id, result_status, result_code, result_body, created_at)
         VALUES ($1, 2, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, NULL, $12, $13, $14,
                 transaction_timestamp())",
    )
    .bind(command.command_id)
    .bind(command.actor_kind())
    .bind(DepositActor::DEPOSIT_SERVICE_REF)
    .bind(command.operation.as_str())
    .bind(command.target.request_target())
    .bind(command.digest.as_bytes().as_slice())
    .bind(command.target.kind())
    .bind(target_id)
    .bind(bot_uid)
    .bind(world_id)
    .bind(external_mid)
    .bind(status)
    .bind(code)
    .bind(body)
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        "INSERT INTO public.audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id, result,
             request_id, source, metadata)
         VALUES ($1, transaction_timestamp(), $2, $3, $4, 'consignment_command', $5, $6,
                 NULL, 'worker', $7)",
    )
    .bind(Uuid::new_v4())
    .bind(command.actor_kind())
    .bind(DepositActor::DEPOSIT_SERVICE_REF)
    .bind(format!("consignment.{}", command.operation.as_str()))
    .bind(command.command_id.to_string())
    .bind(if status < 400 { "success" } else { "rejected" })
    .bind(json!({ "command_id": command.command_id, "disposition": disposition }))
    .execute(&mut *connection)
    .await
    .record_audit_failure()?;
    Ok(())
}
