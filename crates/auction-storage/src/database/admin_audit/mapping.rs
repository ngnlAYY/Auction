//! SQLite Admin 审计行、安全投影和时间格式映射。

use super::*;
use auction_application::AdminAuditRecord;

#[cfg(feature = "sqlite")]
pub(super) fn map_sqlite_record(
    row: sqlx::sqlite::SqliteRow,
) -> Result<AdminAuditRecord, ApplicationError> {
    use serde_json::Value;
    use sqlx::Row;
    use uuid::Uuid;

    let id: Vec<u8> =
        row.try_get("id").map_err(|_| storage_error("读取 SQLite Admin 审计标识失败"))?;
    let id = Uuid::from_slice(&id).map_err(|_| storage_error("SQLite Admin 审计标识格式无效"))?;
    let timestamp: String =
        row.try_get("occurred_at").map_err(|_| storage_error("读取 SQLite Admin 审计时间失败"))?;
    let occurred_at = parse_sqlite_timestamp(&timestamp)?;
    let metadata_text: String = row
        .try_get("metadata")
        .map_err(|_| storage_error("读取 SQLite Admin 审计 metadata 失败"))?;
    let metadata: Value = serde_json::from_str(&metadata_text)
        .map_err(|_| storage_error("SQLite Admin 审计 metadata 格式无效"))?;
    let record = AdminAuditRecord {
        id,
        occurred_at,
        actor_type: row
            .try_get("actor_type")
            .map_err(|_| storage_error("读取 SQLite Admin 审计 actor 类型失败"))?,
        actor_id: row
            .try_get("actor_id")
            .map_err(|_| storage_error("读取 SQLite Admin 审计 actor 标识失败"))?,
        action: row
            .try_get("action")
            .map_err(|_| storage_error("读取 SQLite Admin 审计动作失败"))?,
        entity_type: row
            .try_get("entity_type")
            .map_err(|_| storage_error("读取 SQLite Admin 审计实体类型失败"))?,
        entity_id: row
            .try_get("entity_id")
            .map_err(|_| storage_error("读取 SQLite Admin 审计实体标识失败"))?,
        result: row
            .try_get("result")
            .map_err(|_| storage_error("读取 SQLite Admin 审计结果失败"))?,
        request_id: row
            .try_get("request_id")
            .map_err(|_| storage_error("读取 SQLite Admin 审计请求标识失败"))?,
        source: row
            .try_get("source")
            .map_err(|_| storage_error("读取 SQLite Admin 审计来源失败"))?,
        metadata,
    };
    if record.validate() {
        Ok(record)
    } else {
        Err(storage_error("SQLite Admin 审计安全投影无效"))
    }
}

#[cfg(feature = "sqlite")]
pub(super) fn sqlite_timestamp(value: chrono::DateTime<chrono::Utc>) -> String {
    value.naive_utc().format("%Y-%m-%d %H:%M:%S%.f").to_string()
}

#[cfg(feature = "sqlite")]
fn parse_sqlite_timestamp(value: &str) -> Result<chrono::DateTime<chrono::Utc>, ApplicationError> {
    use chrono::{DateTime, NaiveDateTime, Utc};
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Ok(parsed.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(value, format) {
            return Ok(DateTime::from_naive_utc_and_offset(parsed, Utc));
        }
    }
    Err(storage_error("SQLite Admin 审计时间格式无效"))
}
