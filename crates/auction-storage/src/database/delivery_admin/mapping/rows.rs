//! Delivery 行读取与旧版完整载荷的防腐映射。

use auction_application::delivery::{
    DELIVERY_ADMIN_RETIREMENT_REASONS, DeliveryAdminError, DeliveryAdminResult,
    DeliveryAdminSnapshot,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Row, postgres::PgRow};
use uuid::Uuid;

use crate::database::delivery_admin::map_storage_error;

use super::evidence::{
    reason_code_from_metadata, returned_mail_id_from_metadata, take_required_from_metadata,
    valid_failed_reference_metadata, valid_preflight_retirement_metadata,
    valid_preparation_rejection_metadata, valid_quarantined_mail_metadata,
    valid_terminal_alert_metadata,
};
use super::status::{
    cod_from_payload, reason_code_filtered, send_category, suppression_reason_for,
    template_from_payload, world_from_payload,
};
use super::validation::{is_zero_time, valid_live_lease, valid_snapshot};

pub(crate) fn snapshot(row: &PgRow) -> Result<DeliveryAdminSnapshot, DeliveryAdminError> {
    let attempt_count: i32 = row.try_get("attempt_count").map_err(map_storage_error)?;
    let attempt_count =
        u32::try_from(attempt_count).map_err(|_| DeliveryAdminError::StoredResultCorrupt)?;
    let lease_token =
        row.try_get::<Option<uuid::Uuid>, _>("lease_token").map_err(map_storage_error)?;
    let payload: Value = row.try_get("payload").map_err(map_storage_error)?;
    let result_metadata: Value = row.try_get("result_metadata").map_err(map_storage_error)?;
    let status: String = row.try_get("status").map_err(map_storage_error)?;
    let origin_kind: String = row.try_get("origin_kind").map_err(map_storage_error)?;
    let sending_at: Option<DateTime<Utc>> = row.try_get("sending_at").map_err(map_storage_error)?;
    let failure_code: Option<String> = row.try_get("failure_code").map_err(map_storage_error)?;
    let reason_code = reason_code_from_metadata(&result_metadata);
    // 只有确定未开始外部写入的终态才能显示退役或抑制原因；进入围栏后的未知结果不改写语义。
    let pre_send_terminal = status == "failed" && sending_at.is_none();
    let retirement_reason = pre_send_terminal
        .then(|| reason_code_filtered(reason_code.as_deref(), &DELIVERY_ADMIN_RETIREMENT_REASONS))
        .flatten();
    let suppression_reason = if pre_send_terminal {
        suppression_reason_for(failure_code.as_deref(), reason_code.as_deref())
    } else {
        None
    };
    // 旧版 CanonicalObject 要求两个持久化 JSON 都是对象；不能把损坏正文降级成空投影。
    if !payload.is_object()
        || !result_metadata.is_object()
        || !valid_terminal_alert_metadata(&result_metadata)
        || !valid_quarantined_mail_metadata(&result_metadata, &status)
        || !valid_preflight_retirement_metadata(&result_metadata)
    {
        return Err(DeliveryAdminError::StoredResultCorrupt);
    }
    let snapshot = DeliveryAdminSnapshot {
        id: row.try_get("id").map_err(map_storage_error)?,
        origin_kind: origin_kind.clone(),
        origin_id: row.try_get("origin_id").map_err(map_storage_error)?,
        recipient: row.try_get("recipient").map_err(map_storage_error)?,
        cod: cod_from_payload(&payload),
        status,
        template: template_from_payload(&payload),
        category: send_category(&origin_kind, &payload),
        world: world_from_payload(&payload),
        retirement_reason,
        suppression_reason,
        lease_expires_at: row.try_get("lease_expires_at").map_err(map_storage_error)?,
        fencing_version: row.try_get("fencing_version").map_err(map_storage_error)?,
        attempt_count,
        claimed_at: row.try_get("claimed_at").map_err(map_storage_error)?,
        sending_at,
        completed_at: row.try_get("completed_at").map_err(map_storage_error)?,
        external_reference: row.try_get("external_reference").map_err(map_storage_error)?,
        failure_code,
        reason_code,
        take_required: take_required_from_metadata(&result_metadata),
        returned_mail_id: returned_mail_id_from_metadata(&result_metadata),
        created_at: row.try_get("created_at").map_err(map_storage_error)?,
        updated_at: row.try_get("updated_at").map_err(map_storage_error)?,
    };
    if !valid_live_lease(&snapshot.status, lease_token.as_ref(), snapshot.lease_expires_at.as_ref())
        || !valid_snapshot(&snapshot)
        || !valid_failed_reference_metadata(&snapshot, &payload, &result_metadata)
        || !valid_preparation_rejection_metadata(&snapshot, &result_metadata)
    {
        return Err(DeliveryAdminError::StoredResultCorrupt);
    }
    Ok(snapshot)
}

/// 从完整 delivery 行生成旧 Go 实例可读取的命令结果，并同时返回安全应用投影。
/// `result_payload` 属于滚动升级协议：旧版写入并读取完整 `delivery.Delivery`，新版不能把精简 HTTP
/// 投影直接持久化。完整正文只在本模块短暂存在，不得跨 storage 边界返回。
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
/// 旧 Go 类型没有 JSON tag，字段名中的 `ID` 必须保持大写缩写，不能交给通用 PascalCase 变成 `Id`；
/// 固定数组保留 Go 默认的数字数组编码，不能改成 UUID 字符串或 hex digest。
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
            || !valid_preflight_retirement_metadata(&self.result_metadata)
            || !valid_activation_snapshot(self)
        {
            return Err(DeliveryAdminError::StoredResultCorrupt);
        }
        let attempt_count = u32::try_from(self.attempt_count)
            .map_err(|_| DeliveryAdminError::StoredResultCorrupt)?;
        let lease_token = self.lease_token.map(Uuid::from_bytes);
        let reason_code = reason_code_from_metadata(&self.result_metadata);
        let pre_send_terminal = self.status == "failed" && self.sending_at.is_none();
        let retirement_reason = pre_send_terminal
            .then(|| {
                reason_code_filtered(reason_code.as_deref(), &DELIVERY_ADMIN_RETIREMENT_REASONS)
            })
            .flatten();
        let suppression_reason = if pre_send_terminal {
            suppression_reason_for(self.failure_code.as_deref(), reason_code.as_deref())
        } else {
            None
        };
        let snapshot = DeliveryAdminSnapshot {
            id: Uuid::from_bytes(self.id),
            origin_kind: self.origin_kind.clone(),
            origin_id: self.origin_id.clone(),
            recipient: self.recipient.clone(),
            cod: cod_from_payload(&self.payload),
            status: self.status.clone(),
            template: template_from_payload(&self.payload),
            category: send_category(&self.origin_kind, &self.payload),
            world: world_from_payload(&self.payload),
            retirement_reason,
            suppression_reason,
            lease_expires_at: self.lease_expires_at,
            fencing_version: self.fencing_version,
            attempt_count,
            claimed_at: self.claimed_at,
            sending_at: self.sending_at,
            completed_at: self.completed_at,
            external_reference: self.external_reference.clone(),
            failure_code: self.failure_code.clone(),
            reason_code,
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
            || !valid_failed_reference_metadata(&snapshot, &self.payload, &self.result_metadata)
            || !valid_preparation_rejection_metadata(&snapshot, &self.result_metadata)
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
