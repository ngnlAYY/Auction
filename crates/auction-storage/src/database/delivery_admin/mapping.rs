//! delivery Admin 安全投影与 payload metadata 映射。

use auction_application::DeliveryAdminSnapshot;
use serde_json::Value;
use sqlx::{Row, postgres::PgRow};

use super::{map_storage_error, repository_error_owned};

pub(crate) fn snapshot(
    row: &PgRow,
) -> Result<DeliveryAdminSnapshot, auction_application::RepositoryError> {
    let attempt_count: i32 = row.try_get("attempt_count").map_err(map_storage_error)?;
    let attempt_count = u32::try_from(attempt_count)
        .map_err(|_| repository_error_owned("delivery 尝试次数无效"))?;
    let result_metadata: Value = row.try_get("result_metadata").map_err(map_storage_error)?;
    let snapshot = DeliveryAdminSnapshot {
        id: row.try_get("id").map_err(map_storage_error)?,
        origin_kind: row.try_get("origin_kind").map_err(map_storage_error)?,
        origin_id: row.try_get("origin_id").map_err(map_storage_error)?,
        recipient: row.try_get("recipient").map_err(map_storage_error)?,
        cod: cod_from_payload(&row.try_get::<Value, _>("payload").map_err(map_storage_error)?),
        status: row.try_get("status").map_err(map_storage_error)?,
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
    if snapshot.id.is_nil()
        || snapshot.origin_kind.is_empty()
        || snapshot.origin_id != snapshot.origin_id.trim()
        || snapshot.origin_id.is_empty()
        || snapshot.recipient != snapshot.recipient.trim()
        || snapshot.recipient.is_empty()
        || snapshot.fencing_version < 0
        || snapshot.updated_at < snapshot.created_at
    {
        return Err(repository_error_owned("delivery Admin 投影不满足不变量"));
    }
    Ok(snapshot)
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
