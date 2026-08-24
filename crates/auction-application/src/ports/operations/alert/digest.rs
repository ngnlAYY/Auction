//! operations alert 正文、请求摘要和 fingerprint 计算。

use super::delivery::OperationsAlertBatchItem;
use super::model::OperationsAlertTarget;
use super::signals::{OperationsAlertError, OperationsAlertKind, OperationsAlertSource};
use super::*;

/// 生成批次正文的稳定单行/多行表示。
pub fn canonical_summary(
    items: &[OperationsAlertBatchItem],
    maximum_body_bytes: usize,
) -> Result<String, OperationsAlertError> {
    if items.is_empty()
        || items.len() > MAX_OPERATIONS_ALERT_BATCH_ITEMS
        || maximum_body_bytes == 0
        || maximum_body_bytes > MAX_OPERATIONS_ALERT_BODY_BYTES
    {
        return Err(OperationsAlertError::Invalid);
    }
    let mut lines = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        if item.ordinal != i32::try_from(index + 1).unwrap_or_default()
            || item.fingerprint == [0; 32]
            || item.signal_version <= 0
            || item.summary.is_empty()
            || item.summary != item.summary.trim()
            || item.summary.len() > 512
            || item.summary.chars().any(char::is_control)
        {
            return Err(OperationsAlertError::StoredResultCorrupt);
        }
        lines.push(format!("- {}", item.summary));
    }
    let result = lines.join("\n");
    if result.len() > maximum_body_bytes {
        return Err(OperationsAlertError::Invalid);
    }
    Ok(result)
}

/// 计算冻结正文摘要。
pub fn body_digest(summary: &str) -> [u8; 32] {
    Sha256::digest(summary.as_bytes()).into()
}

/// 计算 operations alert delivery 使用的稳定请求摘要。
pub fn request_hash(
    batch_id: Uuid,
    delivery_id: Uuid,
    command_id: Uuid,
    target: &OperationsAlertTarget,
    summary: &str,
) -> Result<RequestHash, ApplicationError> {
    #[derive(Serialize)]
    struct Request<'a> {
        batch_id: Uuid,
        delivery_id: Uuid,
        command_id: Uuid,
        world_id: &'a str,
        recipient: &'a str,
        summary: &'a str,
    }
    target.validate().map_err(ApplicationError::OperationsAlert)?;
    if summary.is_empty() || summary.len() > MAX_OPERATIONS_ALERT_BODY_BYTES {
        return Err(OperationsAlertError::Invalid.into());
    }
    RequestHash::from_request(
        "operations_alert_delivery",
        batch_id,
        &Request {
            batch_id,
            delivery_id,
            command_id,
            world_id: &target.world_id,
            recipient: &target.recipient,
            summary,
        },
    )
    .map_err(ApplicationError::from)
}

pub(super) fn signal_fingerprint(kind: OperationsAlertKind, scope: &str) -> [u8; 32] {
    Sha256::digest(format!("operations-alert:v1\0{}\0{scope}", kind.as_str()).as_bytes()).into()
}

pub(super) fn valid_signal_identity(
    kind: OperationsAlertKind,
    scope: &str,
    source: OperationsAlertSource,
) -> bool {
    match kind {
        OperationsAlertKind::ServiceMissing | OperationsAlertKind::ServiceDegraded => {
            source == OperationsAlertSource::Process
                && matches!(scope, "public" | "admin" | "worker")
        }
        OperationsAlertKind::AuditFailure => {
            source == OperationsAlertSource::Process && scope == "audit"
        }
        OperationsAlertKind::SourceUnavailable => scope == source.as_str(),
        OperationsAlertKind::DeliveryPending | OperationsAlertKind::DeliveryManualReconcile => {
            source == OperationsAlertSource::Delivery && scope == "delivery"
        }
        OperationsAlertKind::FulfillmentBacklog
        | OperationsAlertKind::FulfillmentFailed
        | OperationsAlertKind::FulfillmentManualReconcile => {
            source == OperationsAlertSource::Fulfillment && scope == "auction"
        }
        OperationsAlertKind::RecoveryPaymentOverdue
        | OperationsAlertKind::RecoveryRelistPending
        | OperationsAlertKind::RecoveryFailed
        | OperationsAlertKind::RecoveryManualReconcile => {
            source == OperationsAlertSource::Recovery && scope == "auction"
        }
        OperationsAlertKind::BalanceLow | OperationsAlertKind::EvidenceStale => {
            source == OperationsAlertSource::Recovery && matches!(scope, "main" | "isekai")
        }
    }
}

pub(super) fn valid_age_threshold(value: Duration) -> bool {
    value >= Duration::from_secs(1)
        && value <= Duration::from_secs(30 * 24 * 3_600)
        && value.subsec_nanos() == 0
}

pub(super) fn valid_health_error_code(value: &str) -> bool {
    matches!(
        value,
        "" | "timeout"
            | "canceled"
            | "persistence"
            | "dependency_mismatch"
            | "delivery_failed"
            | "delivery_manual_reconcile"
            | "retry_exhausted"
    )
}
