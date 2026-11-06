//! Operations alert PostgreSQL 行到领域批次模型的安全映射。

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

use crate::database::operations_alert::storage_error;
use auction_application::{
    ApplicationError, OperationsAlertBatchItem, OperationsAlertError, OperationsAlertKind,
    OperationsAlertSignal, OperationsAlertSource, RequestHash,
};
use sqlx::Row;

pub(super) fn signal_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<OperationsAlertSignal, ApplicationError> {
    let fingerprint: Vec<u8> = row.try_get("fingerprint").map_err(|error| {
        storage_error_from(error, "读取 operations alert signal fingerprint 失败")
    })?;
    let kind: String = row
        .try_get("kind")
        .map_err(|error| storage_error_from(error, "读取 operations alert signal 类型失败"))?;
    let scope: String = row
        .try_get("scope")
        .map_err(|error| storage_error_from(error, "读取 operations alert signal 作用域失败"))?;
    let source: String = row
        .try_get("source")
        .map_err(|error| storage_error_from(error, "读取 operations alert signal 来源失败"))?;
    let count: i64 = row
        .try_get("count_value")
        .map_err(|error| storage_error_from(error, "读取 operations alert signal 计数失败"))?;
    let age_seconds: i64 = row
        .try_get("age_seconds")
        .map_err(|error| storage_error_from(error, "读取 operations alert signal 年龄失败"))?;
    let kind =
        parse_kind(&kind).ok_or_else(|| storage_error("operations alert signal 类型未知"))?;
    let source =
        parse_source(&source).ok_or_else(|| storage_error("operations alert signal 来源未知"))?;
    let signal = OperationsAlertSignal::new(kind, scope, source, count, age_seconds)
        .map_err(ApplicationError::from)?;
    if signal.fingerprint.as_slice() != fingerprint.as_slice() {
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::StoredResultCorrupt));
    }
    Ok(signal)
}

pub(super) fn map_batch_items(
    rows: Vec<sqlx::postgres::PgRow>,
) -> Result<Vec<OperationsAlertBatchItem>, ApplicationError> {
    let mut result = Vec::with_capacity(rows.len());
    for row in rows {
        let fingerprint = digest_from_bytes(row.try_get("fingerprint").map_err(|error| {
            storage_error_from(error, "读取 operations alert 条目 fingerprint 失败")
        })?)?;
        result.push(OperationsAlertBatchItem {
            fingerprint,
            signal_version: row
                .try_get("signal_version")
                .map_err(|error| storage_error_from(error, "读取 operations alert 条目版本失败"))?,
            ordinal: row
                .try_get("ordinal")
                .map_err(|error| storage_error_from(error, "读取 operations alert 条目序号失败"))?,
            summary: row
                .try_get("rendered_summary")
                .map_err(|error| storage_error_from(error, "读取 operations alert 条目摘要失败"))?,
        });
    }
    if result.is_empty() {
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::StoredResultCorrupt));
    }
    Ok(result)
}

pub(super) fn digest_from_bytes(value: Vec<u8>) -> Result<[u8; 32], ApplicationError> {
    value
        .try_into()
        .map_err(|_| ApplicationError::OperationsAlert(OperationsAlertError::StoredResultCorrupt))
}

pub(super) fn request_hash_from_bytes(value: Vec<u8>) -> Result<RequestHash, ApplicationError> {
    Ok(RequestHash::new(digest_from_bytes(value)?))
}

pub(super) fn parse_kind(value: &str) -> Option<OperationsAlertKind> {
    // 闭集由应用层 `OperationsAlertKind::ALL` 唯一拥有；此处不重复维护名单，
    // 否则新增告警来源会在领取时被误判为“类型未知”而永久无法通知。
    OperationsAlertKind::parse(value)
}

pub(super) fn parse_source(value: &str) -> Option<OperationsAlertSource> {
    OperationsAlertSource::parse(value)
}
