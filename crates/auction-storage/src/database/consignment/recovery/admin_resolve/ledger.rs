//! Admin 订单命令的账本仲裁：同命令 ID 同请求回放原回执，异请求冲突，确定拒绝只占该次命令 ID。
//!
//! 保护不变量 1、3、6、8：回执、类型化目标与审计同事务追加；`command_id_conflict` 不入账。
//! recheck / resolve 不在 `DepositOperation` 闭集内（该闭集与 `DepositWriter` 是集成人的串行点），
//! 所以本模块沿派发账本的先例直接对 `consignment_commands` 做同形的仲裁与追加，列与 CHECK 由
//! 000137 的 7E-2 追加段定义（admin actor、`contract` 目标、回执形状）。

use auction_application::consignment::{
    ConsignmentError, DepositBusinessRejection, ListingBusinessRejection, OrderAdminOperation,
};
use auction_application::core::RequestHash;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::dispatch::lock_order::lock_listing_and_position;
use crate::database::consignment::reads::orders::ORDER_STREAM_PREFIX;
use crate::database::operations::audit_health::AuditWriteResult as _;

/// 本命令族可持久化的确定拒绝；码与状态必须落在 000136 / 000137 的回执白名单内。
pub(super) const REJECTIONS: [DepositBusinessRejection; 6] = [
    DepositBusinessRejection::StaleVersion,
    DepositBusinessRejection::EvidenceUnproven,
    DepositBusinessRejection::EvidenceConflict,
    DepositBusinessRejection::ActionNotAllowed,
    DepositBusinessRejection::OppositePaymentOpen,
    DepositBusinessRejection::DispositionConflict,
];

/// 一条待仲裁的订单命令身份。
#[derive(Debug, Clone, Copy)]
pub(super) struct OrderLedger<'a> {
    pub(super) command_id: Uuid,
    pub(super) operation: OrderAdminOperation,
    pub(super) actor: &'a str,
    pub(super) contract_id: Uuid,
    pub(super) digest: RequestHash,
    pub(super) request_id: Option<&'a str>,
}

/// 事务内一步的结果：完成，或已把确定拒绝写入账本、待提交后报告。
pub(super) enum Flow<T> {
    /// 业务事实与成功回执已写入（或原回执已回放）。
    Done(T),
    /// 确定拒绝已追加到账本。
    Rejected(DepositBusinessRejection),
}

impl<T> Flow<T> {
    /// 提交之后把账本里的确定拒绝还原成应用错误。
    pub(super) fn into_result(self) -> Result<T, ConsignmentError> {
        match self {
            Self::Done(value) => Ok(value),
            Self::Rejected(rejection) => Err(ConsignmentError::Deposit(rejection)),
        }
    }
}

/// 账本里已有的同命令结果。
pub(super) enum Prior {
    /// 没有这条命令，本事务首次写入。
    Fresh,
    /// 成功回执正文，原样回放。
    Receipt(Value),
    /// 已存的确定拒绝，按原样重放为错误。
    Rejected(DepositBusinessRejection),
}

/// 不变量 1：先核身份与摘要。同 ID 异请求冲突且不写账本。
pub(super) async fn prior(
    connection: &mut PgConnection,
    ledger: &OrderLedger<'_>,
) -> Result<Prior, WriteError> {
    let row = sqlx::query(
        "SELECT schema_version, actor_kind, actor_ref, operation, request_target, request_digest,
                target_kind, target_id, result_status, result_code, result_body
         FROM public.consignment_commands WHERE command_id = $1",
    )
    .bind(ledger.command_id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(row) = row else {
        return Ok(Prior::Fresh);
    };
    let digest: &[u8] = row.try_get("request_digest")?;
    let same_identity = row.try_get::<i16, _>("schema_version")? == 2
        && row.try_get::<&str, _>("actor_kind")? == "admin"
        && row.try_get::<&str, _>("actor_ref")? == ledger.actor
        && row.try_get::<&str, _>("operation")? == ledger.operation.as_str()
        && row.try_get::<&str, _>("request_target")? == ledger.contract_id.to_string()
        && row.try_get::<Option<&str>, _>("target_kind")? == Some("contract")
        && row.try_get::<Option<Uuid>, _>("target_id")? == Some(ledger.contract_id)
        && digest.len() == 32
        && bool::from(digest.ct_eq(ledger.digest.as_bytes().as_slice()));
    if !same_identity {
        return Err(ConsignmentError::CommandIdConflict.into());
    }
    if row.try_get::<i16, _>("result_status")? >= 400 {
        let code: String = row.try_get("result_code")?;
        let rejection = REJECTIONS
            .into_iter()
            .find(|rejection| rejection.code() == code)
            .ok_or_else(|| WriteError::invariant("consignment_order_rejection_code"))?;
        return Ok(Prior::Rejected(rejection));
    }
    Ok(Prior::Receipt(row.try_get("result_body")?))
}

/// 合同此刻的版本：订单阶段事件流的头部序号，没有事件时为 1（与订单读取同口径）。
pub(super) async fn contract_version(
    connection: &mut PgConnection,
    contract_id: Uuid,
) -> Result<u64, WriteError> {
    let head: Option<i64> = sqlx::query_scalar(
        "SELECT next_sequence - 1 FROM public.realtime_streams WHERE stream_key = $1",
    )
    .bind(format!("{ORDER_STREAM_PREFIX}{contract_id}"))
    .fetch_optional(&mut *connection)
    .await?;
    Ok(head.map_or(1, |head| head.max(1)).cast_unsigned())
}

/// 写入成功回执；`fields` 是该操作的必有字段，公共头由本函数补齐。
pub(super) async fn insert_success(
    connection: &mut PgConnection,
    ledger: &OrderLedger<'_>,
    status: i16,
    disposition: &str,
    fields: Value,
) -> Result<Value, WriteError> {
    let mut body = json!({
        "schema_version": 2,
        "operation": ledger.operation.as_str(),
        "disposition": disposition,
        "command_id": ledger.command_id.to_string(),
        "contract_id": ledger.contract_id.to_string(),
    });
    if let (Some(target), Value::Object(extra)) = (body.as_object_mut(), fields) {
        target.extend(extra);
    }
    insert(connection, ledger, status, "ok", &body, disposition).await?;
    Ok(body)
}

/// 追加确定拒绝；确定拒绝只占该次命令 ID，不写任何业务事实。
pub(super) async fn insert_rejection<T>(
    connection: &mut PgConnection,
    ledger: &OrderLedger<'_>,
    rejection: DepositBusinessRejection,
) -> Result<Flow<T>, WriteError> {
    let body = json!({
        "schema_version": 2,
        "operation": ledger.operation.as_str(),
        "disposition": "rejected",
        "command_id": ledger.command_id.to_string(),
        "code": rejection.code(),
    });
    let status = i16::try_from(rejection.status())?;
    insert(connection, ledger, status, rejection.code(), &body, "rejected").await?;
    Ok(Flow::Rejected(rejection))
}

/// 账本行与审计行同事务追加；审计只记命令与处置码，不含装备、预约码或凭据。
async fn insert(
    connection: &mut PgConnection,
    ledger: &OrderLedger<'_>,
    status: i16,
    code: &str,
    body: &Value,
    disposition: &str,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ($1, 2, 'admin', $2, $3, $4, $5, 'contract', $6, NULL, $7, $8, $9,
                 transaction_timestamp())",
    )
    .bind(ledger.command_id)
    .bind(ledger.actor)
    .bind(ledger.operation.as_str())
    .bind(ledger.contract_id.to_string())
    .bind(ledger.digest.as_bytes().as_slice())
    .bind(ledger.contract_id)
    .bind(status)
    .bind(code)
    .bind(body)
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        "INSERT INTO public.audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id, result,
             request_id, source, metadata)
         VALUES ($1, transaction_timestamp(), 'admin', $2, $3, 'consignment_command', $4, $5, $6,
                 'admin', $7)",
    )
    .bind(Uuid::new_v4())
    .bind(ledger.actor)
    .bind(format!("consignment.{}", ledger.operation.as_str()))
    .bind(ledger.command_id.to_string())
    .bind(if status < 400 { "success" } else { "rejected" })
    .bind(ledger.request_id)
    .bind(json!({ "command_id": ledger.command_id, "disposition": disposition }))
    .execute(&mut *connection)
    .await
    .record_audit_failure()?;
    Ok(())
}

/// 取得合同锁之后的准入结果。
pub(super) enum Admission {
    /// 同命令的成功回执，原样回放。
    Replayed(Value),
    /// 首次写入；携带准入时的合同版本。
    Fresh(u64),
}

/// 不变量 1、2、6：先仲裁账本，再按“挂单 → position”顺序取行锁，最后核对预期版本。
///
/// `expected_version` 为空时不核版本：退款类处置的资金写端已在前一事务提交，落账事务不得再因版本漂移
/// 拒绝一件已经发生的资金事实（版本只在决定是否开始资金动作的那一次核对）。
pub(super) async fn admit(
    connection: &mut PgConnection,
    ledger: &OrderLedger<'_>,
    expected_version: Option<u64>,
) -> Result<Flow<Admission>, WriteError> {
    match prior(connection, ledger).await? {
        Prior::Receipt(body) => return Ok(Flow::Done(Admission::Replayed(body))),
        Prior::Rejected(rejection) => return Ok(Flow::Rejected(rejection)),
        Prior::Fresh => {}
    }
    // 合同不存在：没有可锁的行，也没有可挂靠的账本目标，按 404 报告且不入账。
    if !lock_listing_and_position(connection, ledger.contract_id).await? {
        return Err(ConsignmentError::Business(ListingBusinessRejection::ListingNotFound).into());
    }
    let version = contract_version(connection, ledger.contract_id).await?;
    if expected_version.is_some_and(|expected| expected != version) {
        return insert_rejection(connection, ledger, DepositBusinessRejection::StaleVersion).await;
    }
    Ok(Flow::Done(Admission::Fresh(version)))
}

/// 回执正文里的必有 UUID 字段；缺失或形状不对说明账本被改坏，失败关闭。
pub(super) fn body_uuid(body: &Value, field: &'static str) -> Result<Uuid, WriteError> {
    body.get(field)
        .and_then(Value::as_str)
        .and_then(|text| Uuid::parse_str(text).ok())
        .ok_or_else(|| WriteError::invariant("consignment_order_receipt_body"))
}

/// 回执正文里的必有版本字段。
pub(super) fn body_version(body: &Value) -> Result<u64, WriteError> {
    body.get("contract_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| WriteError::invariant("consignment_order_receipt_body"))
}
