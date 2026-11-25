//! 受限权利命令在全局命令账本里的仲裁与回执（actor `browser` / `right_holder`，J5）。
//!
//! 不变量 1、3、8：同命令 ID 同请求返回首次回执，同 ID 不同 actor / 操作 / 目标 / 摘要是冲突；
//! 首次回执（含 403 / 409 拒绝）与业务行同事务追加，只追加不改写。账本的 CHECK 由迁移 7M-2 追加
//! 本模块使用的 actor、target 种类与回执形状（见交付报告的“交给集成人”），之前数据库拒绝，失败关闭。

use auction_application::consignment::RightsStoreError;
use auction_time::Timestamp;
use serde_json::{Map, Value, json};
use sqlx::{PgConnection, Row};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use super::TxError;
use crate::database::operations::audit_health::AuditWriteResult as _;

/// 账本里的 actor 与目标。
#[derive(Clone, Copy)]
pub(super) enum Subject {
    /// 原浏览器：actor_ref 与目标都是浏览器绑定摘要的小写十六进制。
    Browser([u8; 32]),
    /// 已验证的权利人：actor_ref 与目标都是 `right_id`。
    RightHolder(Uuid),
}

impl Subject {
    const fn actor_kind(self) -> &'static str {
        match self {
            Self::Browser(_) => "browser",
            Self::RightHolder(_) => "right_holder",
        }
    }

    const fn target_kind(self) -> &'static str {
        match self {
            Self::Browser(_) => "browser",
            Self::RightHolder(_) => "right",
        }
    }

    fn reference(self) -> String {
        match self {
            Self::Browser(binding) => hex(&binding),
            Self::RightHolder(right_id) => right_id.to_string(),
        }
    }

    const fn target_id(self) -> Option<Uuid> {
        match self {
            Self::Browser(_) => None,
            Self::RightHolder(right_id) => Some(right_id),
        }
    }
}

/// 一条权利命令的账本身份。
#[derive(Clone, Copy)]
pub(super) struct LedgerKey {
    pub(super) command_id: Uuid,
    pub(super) subject: Subject,
    pub(super) operation: &'static str,
    pub(super) request_digest: [u8; 32],
}

/// 已落账的首次回执。
pub(super) struct StoredReceipt {
    pub(super) body: Value,
}

impl StoredReceipt {
    pub(super) fn disposition(&self) -> Option<&str> {
        self.body.get("disposition").and_then(Value::as_str)
    }

    /// 拒绝回执携带的固定错误码；成功回执没有。
    pub(super) fn rejection(&self) -> Option<RightsStoreError> {
        match self.body.get("code").and_then(Value::as_str)? {
            "challenge_invalid" => Some(RightsStoreError::ChallengeInvalid),
            "action_not_allowed" => Some(RightsStoreError::ActionNotAllowed),
            "stale_version" => Some(RightsStoreError::StaleVersion),
            "right_expired" => Some(RightsStoreError::RightExpired),
            _ => None,
        }
    }

    pub(super) fn uuid(&self, field: &str) -> Result<Uuid, TxError> {
        self.body
            .get(field)
            .and_then(Value::as_str)
            .and_then(|text| Uuid::parse_str(text).ok())
            .ok_or_else(corrupt)
    }

    pub(super) fn version(&self, field: &str) -> Result<u64, TxError> {
        self.body.get(field).and_then(Value::as_u64).ok_or_else(corrupt)
    }

    pub(super) fn timestamp(&self, field: &str) -> Result<Timestamp, TxError> {
        self.body
            .get(field)
            .and_then(Value::as_str)
            .and_then(|text| Timestamp::parse_from_rfc3339(text).ok())
            .ok_or_else(corrupt)
    }
}

/// 已落账回执形状损坏：失败关闭，不替调用方猜测。
fn corrupt() -> TxError {
    tracing::warn!("受限权利命令回执形状损坏");
    TxError::Store(RightsStoreError::Unavailable)
}

/// 先于一切业务判断：同 ID 同请求返回原回执，不同请求冲突，没有则是首次执行。
pub(super) async fn arbitrate(
    connection: &mut PgConnection,
    key: &LedgerKey,
) -> Result<Option<StoredReceipt>, TxError> {
    let row = sqlx::query(
        "SELECT actor_kind, actor_ref, operation, request_target, request_digest,
                result_body
         FROM public.consignment_commands WHERE command_id = $1",
    )
    .bind(key.command_id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let digest: &[u8] = row.try_get("request_digest")?;
    let same = row.try_get::<&str, _>("actor_kind")? == key.subject.actor_kind()
        && row.try_get::<&str, _>("actor_ref")? == key.subject.reference()
        && row.try_get::<&str, _>("operation")? == key.operation
        && row.try_get::<&str, _>("request_target")? == key.subject.reference()
        && digest.len() == 32
        && bool::from(digest.ct_eq(key.request_digest.as_slice()));
    if !same {
        return Err(RightsStoreError::CommandIdConflict.into());
    }
    Ok(Some(StoredReceipt { body: row.try_get("result_body")? }))
}

/// 回执公共头：schema 2、命令 ID、操作名与处置；`extra` 追加业务字段。
pub(super) fn body(key: &LedgerKey, disposition: &str, extra: Value) -> Value {
    let mut map = Map::new();
    map.insert("schema_version".into(), json!(2));
    map.insert("command_id".into(), json!(key.command_id));
    map.insert("operation".into(), json!(key.operation));
    map.insert("disposition".into(), json!(disposition));
    if let Value::Object(fields) = extra {
        map.extend(fields);
    }
    Value::Object(map)
}

/// 追加首次回执。成功回执 `code` 为 `ok`；拒绝回执 `disposition` 为 `rejected` 并带固定错误码。
pub(super) async fn record(
    connection: &mut PgConnection,
    key: &LedgerKey,
    status: u16,
    code: &str,
    receipt: Value,
    now: Timestamp,
) -> Result<(), TxError> {
    sqlx::query(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, replay_key_id, listing_id, target_kind, target_id,
             result_status, result_code, result_body, created_at)
         VALUES ($1, 2, $2, $3, $4, $5, $6, NULL, NULL, $7, $8, $9, $10, $11, $12)",
    )
    .bind(key.command_id)
    .bind(key.subject.actor_kind())
    .bind(key.subject.reference())
    .bind(key.operation)
    .bind(key.subject.reference())
    .bind(key.request_digest.as_slice())
    .bind(key.subject.target_kind())
    .bind(key.subject.target_id())
    .bind(i16::try_from(status)?)
    .bind(code)
    .bind(&receipt)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    audit(connection, key, status, code, &receipt, now).await
}

/// 与回执同事务的追加审计（D9 第 41 行；不变量 3、7、8）。
///
/// actor 归入既有的 `public_user` / `public` 分类（Admin 审计读口只接受既有四类）；actor_id 只写
/// `right_holder:<right_id>` 或固定的 `browser`，浏览器绑定摘要不出账本。元数据只放 right_id、
/// command_id 与结果码，挑战码、定位码与收件信息一律不进审计。
async fn audit(
    connection: &mut PgConnection,
    key: &LedgerKey,
    status: u16,
    code: &str,
    receipt: &Value,
    now: Timestamp,
) -> Result<(), TxError> {
    let right_id = match key.subject {
        Subject::RightHolder(right_id) => Some(right_id),
        Subject::Browser(_) => receipt
            .get("right_id")
            .and_then(Value::as_str)
            .and_then(|text| Uuid::parse_str(text).ok()),
    };
    let actor_id = match key.subject {
        Subject::RightHolder(right_id) => format!("right_holder:{right_id}"),
        Subject::Browser(_) => "browser".to_owned(),
    };
    let (entity_type, entity_id) = match right_id {
        Some(right_id) => ("consignment_right", right_id),
        None => ("consignment_command", key.command_id),
    };
    let mut metadata = Map::new();
    metadata.insert("command_id".into(), json!(key.command_id));
    metadata.insert("result_code".into(), json!(code));
    if let Some(right_id) = right_id {
        metadata.insert("right_id".into(), json!(right_id));
    }
    sqlx::query(
        "INSERT INTO public.audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id, result,
             request_id, source, metadata)
         VALUES ($1, $2, 'public_user', $3, $4, $5, $6, $7, NULL, 'public', $8)",
    )
    .bind(Uuid::new_v4())
    .bind(now)
    .bind(actor_id)
    .bind(format!("consignment.right.{}", key.operation.trim_end_matches("_right")))
    .bind(entity_type)
    .bind(entity_id.to_string())
    .bind(if status < 400 { "success" } else { "rejected" })
    .bind(Value::Object(metadata))
    .execute(connection)
    .await
    .record_audit_failure()?;
    Ok(())
}

/// 拒绝回执：状态码与错误码取端口的固定映射。
pub(super) async fn record_rejection(
    connection: &mut PgConnection,
    key: &LedgerKey,
    error: RightsStoreError,
    now: Timestamp,
) -> Result<(), TxError> {
    let (status, code) = error.api_error();
    let receipt = body(key, "rejected", json!({ "code": code }));
    record(connection, key, status, code, receipt, now).await
}

fn hex(bytes: &[u8; 32]) -> String {
    let mut text = String::with_capacity(64);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}
