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
    let mut body_bytes = 0;
    for (index, item) in items.iter().enumerate() {
        validate_summary_item(item, index)?;
        body_bytes += item.summary.len() + 2 + usize::from(index > 0);
    }
    if body_bytes > maximum_body_bytes {
        return Err(OperationsAlertError::Invalid);
    }
    Ok(render_summary(items, body_bytes))
}

/// 一次选择原逐项删尾流程可接受的最大非空前缀；不改序号或重写正文。
/// 未纳入的后缀仍由持久队列保留；首项无效时沿用原错误分类。
pub fn canonical_summary_prefix(
    items: &[OperationsAlertBatchItem],
    maximum_body_bytes: usize,
) -> Result<(usize, String), OperationsAlertError> {
    if items.is_empty()
        || maximum_body_bytes == 0
        || maximum_body_bytes > MAX_OPERATIONS_ALERT_BODY_BYTES
    {
        return Err(OperationsAlertError::Invalid);
    }
    let mut count = 0;
    let mut body_bytes = 0;
    for (index, item) in items.iter().take(MAX_OPERATIONS_ALERT_BATCH_ITEMS).enumerate() {
        if let Err(error) = validate_summary_item(item, index) {
            if count == 0 {
                return Err(error);
            }
            break;
        }
        let next_body_bytes = body_bytes + item.summary.len() + 2 + usize::from(index > 0);
        if next_body_bytes > maximum_body_bytes {
            break;
        }
        count += 1;
        body_bytes = next_body_bytes;
    }
    if count == 0 {
        return Err(OperationsAlertError::Invalid);
    }
    Ok((count, render_summary(&items[..count], body_bytes)))
}

fn validate_summary_item(
    item: &OperationsAlertBatchItem,
    index: usize,
) -> Result<(), OperationsAlertError> {
    if i32::try_from(index).ok().and_then(|index| index.checked_add(1)) != Some(item.ordinal)
        || item.fingerprint == [0; 32]
        || item.signal_version <= 0
        || !auction_domain::valid_text(&item.summary, 512)
    {
        return Err(OperationsAlertError::StoredResultCorrupt);
    }
    Ok(())
}

fn render_summary(items: &[OperationsAlertBatchItem], body_bytes: usize) -> String {
    let mut summary = String::with_capacity(body_bytes);
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            summary.push('\n');
        }
        summary.push_str("- ");
        summary.push_str(&item.summary);
    }
    summary
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
        OperationsAlertKind::DeliveryPending
        | OperationsAlertKind::DeliveryReturnedMail
        | OperationsAlertKind::DeliveryManualReconcile => {
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
