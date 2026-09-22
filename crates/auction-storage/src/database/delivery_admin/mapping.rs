//! delivery Admin 安全投影与 payload metadata 映射。

use auction_application::{DeliveryAdminError, DeliveryAdminResult, DeliveryAdminSnapshot};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Row, postgres::PgRow};
use uuid::Uuid;

use super::map_storage_error;

pub(crate) fn snapshot(row: &PgRow) -> Result<DeliveryAdminSnapshot, DeliveryAdminError> {
    let attempt_count: i32 = row.try_get("attempt_count").map_err(map_storage_error)?;
    let attempt_count =
        u32::try_from(attempt_count).map_err(|_| DeliveryAdminError::StoredResultCorrupt)?;
    let lease_token =
        row.try_get::<Option<uuid::Uuid>, _>("lease_token").map_err(map_storage_error)?;
    let payload: Value = row.try_get("payload").map_err(map_storage_error)?;
    let result_metadata: Value = row.try_get("result_metadata").map_err(map_storage_error)?;
    let status: String = row.try_get("status").map_err(map_storage_error)?;
    // 旧版 CanonicalObject 要求两个持久化 JSON 都是对象；不能把损坏正文降级成空投影。
    if !payload.is_object()
        || !result_metadata.is_object()
        || !valid_terminal_alert_metadata(&result_metadata)
        || !valid_quarantined_mail_metadata(&result_metadata, &status)
    {
        return Err(DeliveryAdminError::StoredResultCorrupt);
    }
    let snapshot = DeliveryAdminSnapshot {
        id: row.try_get("id").map_err(map_storage_error)?,
        origin_kind: row.try_get("origin_kind").map_err(map_storage_error)?,
        origin_id: row.try_get("origin_id").map_err(map_storage_error)?,
        recipient: row.try_get("recipient").map_err(map_storage_error)?,
        cod: cod_from_payload(&payload),
        status,
        lease_expires_at: row.try_get("lease_expires_at").map_err(map_storage_error)?,
        fencing_version: row.try_get("fencing_version").map_err(map_storage_error)?,
        attempt_count,
        claimed_at: row.try_get("claimed_at").map_err(map_storage_error)?,
        sending_at: row.try_get("sending_at").map_err(map_storage_error)?,
        completed_at: row.try_get("completed_at").map_err(map_storage_error)?,
        external_reference: row.try_get("external_reference").map_err(map_storage_error)?,
        failure_code: row.try_get("failure_code").map_err(map_storage_error)?,
        reason_code: reason_code_from_metadata(&result_metadata),
        take_required: take_required_from_metadata(&result_metadata),
        returned_mail_id: returned_mail_id_from_metadata(&result_metadata),
        created_at: row.try_get("created_at").map_err(map_storage_error)?,
        updated_at: row.try_get("updated_at").map_err(map_storage_error)?,
    };
    if !valid_live_lease(&snapshot.status, lease_token.as_ref(), snapshot.lease_expires_at.as_ref())
        || !valid_snapshot(&snapshot)
    {
        return Err(DeliveryAdminError::StoredResultCorrupt);
    }
    Ok(snapshot)
}

/// 从完整 delivery 行生成旧 Go 实例可读取的命令结果，并同时返回安全应用投影。
///
/// `result_payload` 属于滚动升级协议：旧版写入并读取完整 `delivery.Delivery`，因此新版不能
/// 把精简 HTTP 投影直接持久化。完整正文只在本模块短暂存在，不得跨 storage 边界返回。
pub(crate) fn legacy_result_payload(
    row: &PgRow,
) -> Result<(Value, DeliveryAdminSnapshot), DeliveryAdminError> {
    let legacy = LegacyDeliveryPayload::from_row(row)?;
    let snapshot = legacy.safe_snapshot()?;
    let payload = serde_json::to_value(&legacy).map_err(|_| DeliveryAdminError::Unavailable)?;
    Ok((payload, snapshot))
}

/// 按旧版完整不变量读取命令事务中的 delivery 行。
pub(crate) fn stored_snapshot(row: &PgRow) -> Result<DeliveryAdminSnapshot, DeliveryAdminError> {
    LegacyDeliveryPayload::from_row(row)?.safe_snapshot()
}

/// 双读旧完整结果与当前精简结果，供未发布版本产生的账本继续稳定重放。
pub(crate) fn decode_result_payload(
    payload: Value,
    command_id: Uuid,
) -> Result<DeliveryAdminResult, DeliveryAdminError> {
    let fields = payload.as_object().ok_or(DeliveryAdminError::StoredResultCorrupt)?;
    if fields.contains_key("ID") || fields.contains_key("CommandID") {
        let legacy: LegacyDeliveryPayload =
            serde_json::from_value(payload).map_err(|_| DeliveryAdminError::StoredResultCorrupt)?;
        let snapshot = legacy.safe_snapshot()?;
        return Ok(DeliveryAdminResult { command_id, snapshot });
    }
    if fields.contains_key("command_id") || fields.contains_key("snapshot") {
        return serde_json::from_value(payload)
            .map_err(|_| DeliveryAdminError::StoredResultCorrupt);
    }
    Err(DeliveryAdminError::StoredResultCorrupt)
}

/// Auction-BF `delivery.Delivery` 的账本防腐映射。
///
/// 旧 Go 类型没有 JSON tag；字段名中的 `ID` 必须保持大写缩写，不能交给通用 PascalCase
/// 规则变成 `Id`。固定数组保留 Go 默认的数字数组编码，不能改成 UUID 字符串或 hex digest。
#[derive(Deserialize, Serialize)]
struct LegacyDeliveryPayload {
    #[serde(rename = "ID")]
    id: [u8; 16],
    #[serde(rename = "CommandID")]
    command_id: [u8; 16],
    #[serde(rename = "RequestDigest")]
    request_digest: [u8; 32],
    #[serde(rename = "ActivationCommandID")]
    activation_command_id: Option<[u8; 16]>,
    #[serde(rename = "ActivationRequestDigest")]
    activation_request_digest: Option<[u8; 32]>,
    #[serde(rename = "OriginKind")]
    origin_kind: String,
    #[serde(rename = "OriginID")]
    origin_id: String,
    #[serde(rename = "Recipient")]
    recipient: String,
    #[serde(rename = "Payload")]
    payload: Value,
    #[serde(rename = "Status")]
    status: String,
    #[serde(rename = "LeaseToken")]
    lease_token: Option<[u8; 16]>,
    #[serde(rename = "LeaseExpiresAt")]
    lease_expires_at: Option<DateTime<Utc>>,
    #[serde(rename = "FencingVersion")]
    fencing_version: i64,
    #[serde(rename = "AttemptCount")]
    attempt_count: i32,
    #[serde(rename = "ClaimedAt")]
    claimed_at: Option<DateTime<Utc>>,
    #[serde(rename = "SendingAt")]
    sending_at: Option<DateTime<Utc>>,
    #[serde(rename = "CompletedAt")]
    completed_at: Option<DateTime<Utc>>,
    #[serde(rename = "ExternalReference")]
    external_reference: Option<String>,
    #[serde(rename = "FailureCode")]
    failure_code: Option<String>,
    #[serde(rename = "ResultMetadata")]
    result_metadata: Value,
    #[serde(rename = "ActivatedAt")]
    activated_at: Option<DateTime<Utc>>,
    #[serde(rename = "CreatedAt")]
    created_at: DateTime<Utc>,
    #[serde(rename = "UpdatedAt")]
    updated_at: DateTime<Utc>,
}

impl LegacyDeliveryPayload {
    fn from_row(row: &PgRow) -> Result<Self, DeliveryAdminError> {
        let id: Uuid = row.try_get("id").map_err(map_storage_error)?;
        let command_id: Uuid = row.try_get("command_id").map_err(map_storage_error)?;
        let request_digest: Vec<u8> = row.try_get("request_digest").map_err(map_storage_error)?;
        let activation_command_id: Option<Uuid> =
            row.try_get("activation_command_id").map_err(map_storage_error)?;
        let activation_request_digest: Option<Vec<u8>> =
            row.try_get("activation_request_digest").map_err(map_storage_error)?;
        let lease_token: Option<Uuid> = row.try_get("lease_token").map_err(map_storage_error)?;
        Ok(Self {
            id: *id.as_bytes(),
            command_id: *command_id.as_bytes(),
            request_digest: fixed_bytes(request_digest)?,
            activation_command_id: activation_command_id.map(|value| *value.as_bytes()),
            activation_request_digest: activation_request_digest.map(fixed_bytes).transpose()?,
            origin_kind: row.try_get("origin_kind").map_err(map_storage_error)?,
            origin_id: row.try_get("origin_id").map_err(map_storage_error)?,
            recipient: row.try_get("recipient").map_err(map_storage_error)?,
            payload: row.try_get("payload").map_err(map_storage_error)?,
            status: row.try_get("status").map_err(map_storage_error)?,
            lease_token: lease_token.map(|value| *value.as_bytes()),
            lease_expires_at: row.try_get("lease_expires_at").map_err(map_storage_error)?,
            fencing_version: row.try_get("fencing_version").map_err(map_storage_error)?,
            attempt_count: row.try_get("attempt_count").map_err(map_storage_error)?,
            claimed_at: row.try_get("claimed_at").map_err(map_storage_error)?,
            sending_at: row.try_get("sending_at").map_err(map_storage_error)?,
            completed_at: row.try_get("completed_at").map_err(map_storage_error)?,
            external_reference: row.try_get("external_reference").map_err(map_storage_error)?,
            failure_code: row.try_get("failure_code").map_err(map_storage_error)?,
            result_metadata: row.try_get("result_metadata").map_err(map_storage_error)?,
            activated_at: row.try_get("activated_at").map_err(map_storage_error)?,
            created_at: row.try_get("created_at").map_err(map_storage_error)?,
            updated_at: row.try_get("updated_at").map_err(map_storage_error)?,
        })
    }

    fn safe_snapshot(&self) -> Result<DeliveryAdminSnapshot, DeliveryAdminError> {
        if self.command_id == [0; 16]
            || self.request_digest == [0; 32]
            || !self.payload.is_object()
            || !self.result_metadata.is_object()
            || !valid_terminal_alert_metadata(&self.result_metadata)
            || !valid_quarantined_mail_metadata(&self.result_metadata, &self.status)
            || !valid_activation_snapshot(self)
        {
            return Err(DeliveryAdminError::StoredResultCorrupt);
        }
        let attempt_count = u32::try_from(self.attempt_count)
            .map_err(|_| DeliveryAdminError::StoredResultCorrupt)?;
        let lease_token = self.lease_token.map(Uuid::from_bytes);
        let snapshot = DeliveryAdminSnapshot {
            id: Uuid::from_bytes(self.id),
            origin_kind: self.origin_kind.clone(),
            origin_id: self.origin_id.clone(),
            recipient: self.recipient.clone(),
            cod: cod_from_payload(&self.payload),
            status: self.status.clone(),
            lease_expires_at: self.lease_expires_at,
            fencing_version: self.fencing_version,
            attempt_count,
            claimed_at: self.claimed_at,
            sending_at: self.sending_at,
            completed_at: self.completed_at,
            external_reference: self.external_reference.clone(),
            failure_code: self.failure_code.clone(),
            reason_code: reason_code_from_metadata(&self.result_metadata),
            take_required: take_required_from_metadata(&self.result_metadata),
            returned_mail_id: returned_mail_id_from_metadata(&self.result_metadata),
            created_at: self.created_at,
            updated_at: self.updated_at,
        };
        if !valid_live_lease(
            &snapshot.status,
            lease_token.as_ref(),
            snapshot.lease_expires_at.as_ref(),
        ) || !valid_snapshot(&snapshot)
        {
            return Err(DeliveryAdminError::StoredResultCorrupt);
        }
        Ok(snapshot)
    }
}

fn fixed_bytes<const N: usize>(value: Vec<u8>) -> Result<[u8; N], DeliveryAdminError> {
    value.try_into().map_err(|_| DeliveryAdminError::StoredResultCorrupt)
}

fn valid_activation_snapshot(payload: &LegacyDeliveryPayload) -> bool {
    let has_none = payload.activation_command_id.is_none()
        && payload.activation_request_digest.is_none()
        && payload.activated_at.is_none();
    let has_all = payload.activation_command_id.is_some()
        && payload.activation_request_digest.is_some()
        && payload.activated_at.is_some();
    if (!has_none && !has_all) || (payload.status == "prepared" && !has_none) {
        return false;
    }
    if has_none {
        return true;
    }
    payload.activation_command_id.is_some_and(|value| value != [0; 16])
        && payload.activation_request_digest.is_some_and(|value| value != [0; 32])
        && payload.activated_at.is_some_and(|value| {
            !is_zero_time(value) && value >= payload.created_at && value <= payload.updated_at
        })
}

fn valid_live_lease(
    status: &str,
    lease_token: Option<&uuid::Uuid>,
    lease_expires_at: Option<&DateTime<Utc>>,
) -> bool {
    let has_lease = lease_token.is_some() && lease_expires_at.is_some();
    let valid_token = lease_token.is_none_or(|value| !value.is_nil());
    match status {
        "claimed" | "sending" => has_lease && valid_token,
        "prepared" | "pending" | "succeeded" | "failed" | "manual_reconcile" => {
            lease_token.is_none() && lease_expires_at.is_none()
        }
        _ => false,
    }
}

pub(crate) fn valid_snapshot(snapshot: &DeliveryAdminSnapshot) -> bool {
    if snapshot.id.is_nil()
        || !valid_origin_kind(&snapshot.origin_kind)
        || !valid_stored_text(&snapshot.origin_id)
        || !valid_stored_text(&snapshot.recipient)
        || snapshot.fencing_version < 0
        || snapshot.attempt_count > i32::MAX as u32
        || is_zero_time(snapshot.created_at)
        || snapshot.updated_at < snapshot.created_at
    {
        return false;
    }

    // 历史资产处置冲突在发送之前隔离，不能为了适配终态投影而伪造 sending 事实。
    if snapshot.reason_code.as_deref() == Some("mail_disposition_conflict") {
        return matches!(snapshot.status.as_str(), "manual_reconcile" | "failed")
            && snapshot.origin_kind == "mail_intake"
            && if snapshot.status == "manual_reconcile" {
                snapshot.failure_code.as_deref() == Some("mail_disposition_conflict")
            } else {
                snapshot.failure_code.as_deref().is_some_and(valid_stored_text)
            }
            && snapshot.lease_expires_at.is_none()
            && snapshot.sending_at.is_none()
            && snapshot.external_reference.is_none()
            && !snapshot.take_required
            && snapshot.returned_mail_id.is_none()
            && snapshot.fencing_version > 0
            && snapshot.claimed_at.is_none_or(|claimed| {
                !is_zero_time(claimed)
                    && claimed >= snapshot.created_at
                    && snapshot.attempt_count > 0
            })
            && snapshot.completed_at.is_some_and(|completed| {
                !is_zero_time(completed)
                    && completed >= snapshot.created_at
                    && snapshot.claimed_at.is_none_or(|claimed| completed >= claimed)
                    && completed <= snapshot.updated_at
            });
    }

    // 告警在激活前确定终止没有 claimed/sending 事实，不能套用已发送投递的时间链。
    if snapshot.status == "failed"
        && snapshot.origin_kind == "operations_alert"
        && snapshot.reason_code.as_deref() == Some("operations_alert_batch_terminal")
    {
        return snapshot.lease_expires_at.is_none()
            && snapshot.claimed_at.is_none()
            && snapshot.sending_at.is_none()
            && snapshot.completed_at.is_some_and(|completed| {
                !is_zero_time(completed)
                    && completed >= snapshot.created_at
                    && completed <= snapshot.updated_at
            })
            && snapshot.external_reference.is_none()
            && !snapshot.take_required
            && snapshot.returned_mail_id.is_none()
            && matches!(
                snapshot.failure_code.as_deref(),
                Some("dependency_mismatch" | "retry_exhausted")
            )
            && snapshot.fencing_version == 0
            && snapshot.attempt_count == 0;
    }

    if snapshot.lease_expires_at.is_some_and(is_zero_time)
        || !valid_timeline(
            snapshot.created_at,
            snapshot.claimed_at,
            snapshot.sending_at,
            snapshot.completed_at,
        )
    {
        return false;
    }

    match snapshot.status.as_str() {
        // 与旧版 ValidStateSnapshot 保持一致：prepared/pending 可以保留非负的
        // fencing/attempt 计数，但不能携带执行中或已完成状态的字段。
        "prepared" | "pending" => {
            snapshot.lease_expires_at.is_none()
                && snapshot.claimed_at.is_none()
                && snapshot.sending_at.is_none()
                && snapshot.completed_at.is_none()
                && snapshot.external_reference.is_none()
                && snapshot.failure_code.is_none()
        }
        "claimed" => {
            snapshot.lease_expires_at.is_some()
                && snapshot.claimed_at.is_some()
                && snapshot.sending_at.is_none()
                && snapshot.completed_at.is_none()
                && snapshot.external_reference.is_none()
                && snapshot.failure_code.is_none()
                && snapshot.fencing_version > 0
                && snapshot.attempt_count > 0
        }
        "sending" => {
            snapshot.lease_expires_at.is_some()
                && snapshot.claimed_at.is_some()
                && snapshot.sending_at.is_some()
                && snapshot.completed_at.is_none()
                && snapshot.external_reference.is_none()
                && snapshot.failure_code.is_none()
                && snapshot.fencing_version > 0
                && snapshot.attempt_count > 0
        }
        "succeeded" => {
            snapshot.lease_expires_at.is_none()
                && snapshot.claimed_at.is_some()
                && snapshot.sending_at.is_some()
                && snapshot.completed_at.is_some()
                && snapshot.external_reference.as_deref().is_some_and(valid_stored_text)
                && snapshot.failure_code.is_none()
                && snapshot.fencing_version > 0
                && snapshot.attempt_count > 0
        }
        "failed" | "manual_reconcile" => {
            snapshot.lease_expires_at.is_none()
                && snapshot.claimed_at.is_some()
                && snapshot.sending_at.is_some()
                && snapshot.completed_at.is_some()
                && snapshot.external_reference.is_none()
                && snapshot.failure_code.as_deref().is_some_and(valid_stored_text)
                && snapshot.fencing_version > 0
                && snapshot.attempt_count > 0
        }
        _ => false,
    }
}

fn valid_terminal_alert_metadata(metadata: &Value) -> bool {
    metadata.get("reason_code").and_then(Value::as_str) != Some("operations_alert_batch_terminal")
        || metadata.get("outcome_unknown").and_then(Value::as_bool) == Some(false)
}

fn valid_quarantined_mail_metadata(metadata: &Value, status: &str) -> bool {
    if metadata.get("reason_code").and_then(Value::as_str) != Some("mail_disposition_conflict") {
        return true;
    }
    if metadata.get("preflight_blocked").and_then(Value::as_bool) != Some(true) {
        return false;
    }
    let outcome_unknown = metadata.get("outcome_unknown").and_then(Value::as_bool);
    let resolution = metadata.get("resolution").and_then(Value::as_str);
    match status {
        "manual_reconcile" => outcome_unknown == Some(true) && resolution.is_none(),
        "failed" => outcome_unknown == Some(false) && resolution == Some("admin_reconciled_failed"),
        _ => false,
    }
}

fn valid_timeline(
    created_at: DateTime<Utc>,
    claimed_at: Option<DateTime<Utc>>,
    sending_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
) -> bool {
    if claimed_at.is_some_and(|value| value < created_at) {
        return false;
    }
    if sending_at.is_some_and(|value| claimed_at.is_none_or(|claimed| value < claimed)) {
        return false;
    }
    completed_at.is_none_or(|value| sending_at.is_some_and(|sending| value >= sending))
}

fn valid_origin_kind(value: &str) -> bool {
    matches!(
        value,
        "settlement"
            | "settlement_recovery_action"
            | "user_key"
            | "manual"
            | "mail_intake"
            | "operations_alert"
    )
}

fn valid_stored_text(value: &str) -> bool {
    !value.is_empty() && value == value.trim()
}

fn is_zero_time(value: DateTime<Utc>) -> bool {
    value.timestamp() == -62_135_596_800 && value.timestamp_subsec_nanos() == 0
}

fn cod_from_payload(payload: &Value) -> Option<i64> {
    if payload.get("schema_version").and_then(Value::as_i64) != Some(1)
        || payload.get("operation").and_then(Value::as_str) != Some("send")
    {
        return None;
    }
    let cod = payload.get("cod").and_then(Value::as_i64)?;
    (0..=auction_application::MAX_DELIVERY_COD).contains(&cod).then_some(cod)
}

fn reason_code_from_metadata(metadata: &Value) -> Option<String> {
    let value = metadata.get("reason_code").and_then(Value::as_str)?;
    let trimmed = value.trim();
    (value == trimmed && !trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn take_required_from_metadata(metadata: &Value) -> bool {
    metadata.get("take_required").and_then(Value::as_bool).unwrap_or(false)
}

fn returned_mail_id_from_metadata(metadata: &Value) -> Option<String> {
    let value = metadata.get("returned_mail_id").and_then(Value::as_str)?;
    if !(1..=32).contains(&value.len())
        || !value.bytes().all(|character| character.is_ascii_digit())
    {
        return None;
    }
    Some(value.to_owned())
}
