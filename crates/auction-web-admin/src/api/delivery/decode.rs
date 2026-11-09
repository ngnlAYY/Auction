//! 投递安全投影的 fail-closed 解码与状态机字段校验。
//! 模板/类别/世界/原因只接受服务端闭集，未知形状、越界数字与不自洽的时间链一律让整页
//! 失败关闭；浏览器不推导状态迁移，也不放宽服务端已判定的终态。

use auction_contracts::{
    AdminDeliveryCommandResponse, AdminDeliveryListResponse, AdminDeliveryResponse,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::api::{Delivery, DeliveryPage, MAX_JSON_SAFE_INTEGER, valid_success_code, valid_uuid};

use super::queue::DeliveryAction;

/// Delivery 路由专用的成功响应外壳；不能用宽松的共享 DTO 接受未知字段。
/// 共享 `auction_contracts::ApiResponse` 同时服务 Public surface，刻意保持开放；delivery
/// 路由要求信封闭集，因此本地保留该泛型外壳。同步义务：`ApiResponse` 的 code/message/data
/// 形状变化时必须同步此处。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StrictDeliveryEnvelope<T> {
    pub(super) code: String,
    pub(super) message: String,
    pub(super) data: T,
}

const MAX_DELIVERY_PAGE_SIZE: i64 = 100;

pub(super) fn decode_delivery_list(
    payload: StrictDeliveryEnvelope<AdminDeliveryListResponse>,
) -> Result<DeliveryPage, ()> {
    if !valid_success_code(&payload.code) || payload.message.is_empty() {
        return Err(());
    }

    let pagination = payload.data.pagination;
    if pagination.page <= 0
        || pagination.page > MAX_JSON_SAFE_INTEGER
        || pagination.page_size <= 0
        || pagination.page_size > MAX_DELIVERY_PAGE_SIZE
        || pagination.total < 0
        || pagination.total > MAX_JSON_SAFE_INTEGER
    {
        return Err(());
    }

    let deliveries =
        payload.data.deliveries.into_iter().map(decode_delivery).collect::<Result<Vec<_>, _>>()?;
    if deliveries.len() as i64 > pagination.page_size {
        return Err(());
    }

    Ok(DeliveryPage { deliveries, pagination })
}

pub(super) fn decode_delivery_command(
    payload: StrictDeliveryEnvelope<AdminDeliveryCommandResponse>,
    requested_delivery_id: &str,
    action: DeliveryAction,
    expected_fencing_version: i64,
) -> Result<Delivery, ()> {
    if !valid_success_code(&payload.code) || payload.message.is_empty() {
        return Err(());
    }

    let delivery = decode_delivery(payload.data.delivery)?;
    if delivery.id != requested_delivery_id
        || delivery.fencing_version <= expected_fencing_version
        || delivery.status
            != match action {
                DeliveryAction::Release
                | DeliveryAction::Retry
                | DeliveryAction::ReconcileRetry => "pending",
                DeliveryAction::ReconcileSucceeded => "succeeded",
                DeliveryAction::ReconcileFailed => "failed",
            }
    {
        return Err(());
    }
    Ok(delivery)
}

pub(super) fn decode_delivery(value: AdminDeliveryResponse) -> Result<Delivery, ()> {
    // W4.5 新增字段同样 fail-closed：模板/类别/世界/原因只接受服务端闭集。
    if !valid_optional_text(value.template.as_deref())
        || value.template.as_deref().is_some_and(|template| template.len() > 64)
        || value.category.as_deref().is_some_and(|category| {
            !matches!(
                category,
                "outbid"
                    | "account"
                    | "intake"
                    | "settlement"
                    | "equipment"
                    | "operations"
                    | "manual"
            )
        })
        || value.world.as_deref().is_some_and(|world| !matches!(world, "main" | "isekai"))
        || !valid_optional_text(value.retirement_reason.as_deref())
        || !valid_optional_text(value.suppression_reason.as_deref())
    {
        return Err(());
    }
    if !valid_uuid(&value.id) {
        return Err(());
    }
    let created_at = parse_delivery_time(&value.created_at).ok_or(())?;
    let updated_at = parse_delivery_time(&value.updated_at).ok_or(())?;
    let lease_expires_at = parse_optional_delivery_time(value.lease_expires_at.as_ref())?;
    let claimed_at = parse_optional_delivery_time(value.claimed_at.as_ref())?;
    let sending_at = parse_optional_delivery_time(value.sending_at.as_ref())?;
    let completed_at = parse_optional_delivery_time(value.completed_at.as_ref())?;
    // 服务端只为从未激活的告警投递使用该终态，时间链从创建直接到完成。
    let before_activation = value.status == "failed"
        && value.origin_kind == "operations_alert"
        && value.reason_code.as_deref() == Some("operations_alert_batch_terminal");
    let disposition_conflict = value.reason_code.as_deref() == Some("mail_disposition_conflict");
    let watch_invalidated = value.origin_kind == "lot_watch"
        && matches!(
            value.reason_code.as_deref(),
            Some("lot_watch_invalidated" | "recipient_renamed")
        );
    let preparation_rejected = value.status == "failed"
        && value.failure_code.as_deref() == Some("invalid_payload")
        && sending_at.is_none();
    let timeline_valid = if preparation_rejected {
        claimed_at.is_some_and(|claimed| {
            !is_zero_delivery_time(claimed)
                && claimed >= created_at
                && completed_at.is_some_and(|completed| {
                    !is_zero_delivery_time(completed)
                        && completed >= claimed
                        && completed <= updated_at
                })
        })
    } else if disposition_conflict || watch_invalidated {
        sending_at.is_none()
            && claimed_at.is_none_or(|claimed| {
                !is_zero_delivery_time(claimed) && claimed >= created_at && value.attempt_count > 0
            })
            && completed_at.is_some_and(|completed| {
                !is_zero_delivery_time(completed)
                    && completed >= created_at
                    && claimed_at.is_none_or(|claimed| completed >= claimed)
                    && completed <= updated_at
            })
    } else if before_activation {
        claimed_at.is_none()
            && sending_at.is_none()
            && completed_at.is_some_and(|completed| {
                !is_zero_delivery_time(completed)
                    && completed >= created_at
                    && completed <= updated_at
            })
    } else {
        valid_timeline(created_at, claimed_at, sending_at, completed_at)
    };

    if !valid_origin_kind(&value.origin_kind)
        || !valid_stored_text(&value.origin_id)
        || !valid_stored_text(&value.recipient)
        || !valid_amount(value.cod)
        || !valid_status(&value.status)
        || value.fencing_version < 0
        || value.fencing_version > MAX_JSON_SAFE_INTEGER
        || i64::from(value.attempt_count) > MAX_JSON_SAFE_INTEGER
        || is_zero_delivery_time(created_at)
        || is_zero_delivery_time(updated_at)
        || updated_at < created_at
        || lease_expires_at.is_some_and(is_zero_delivery_time)
        || !timeline_valid
        || !valid_optional_text(value.external_reference.as_deref())
        || !valid_optional_text(value.failure_code.as_deref())
        || !valid_optional_text(value.reason_code.as_deref())
        || !valid_returned_mail_id(value.returned_mail_id.as_deref())
        || !valid_state_fields(&value, lease_expires_at, claimed_at, sending_at, completed_at)
    {
        return Err(());
    }

    Ok(Delivery {
        id: value.id,
        origin_kind: value.origin_kind,
        origin_id: value.origin_id,
        recipient: value.recipient,
        cod: value.cod,
        status: value.status,
        template: value.template,
        category: value.category,
        world: value.world,
        retirement_reason: value.retirement_reason,
        suppression_reason: value.suppression_reason,
        lease_expires_at: value.lease_expires_at,
        fencing_version: value.fencing_version,
        attempt_count: value.attempt_count,
        claimed_at: value.claimed_at,
        sending_at: value.sending_at,
        completed_at: value.completed_at,
        external_reference: value.external_reference,
        failure_code: value.failure_code,
        reason_code: value.reason_code,
        take_required: value.take_required,
        returned_mail_id: value.returned_mail_id,
        created_at: value.created_at,
        updated_at: value.updated_at,
    })
}

pub(super) fn parse_delivery_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value).ok().map(|value| value.with_timezone(&Utc))
}

fn parse_optional_delivery_time(value: Option<&String>) -> Result<Option<DateTime<Utc>>, ()> {
    value.map_or(Ok(None), |value| parse_delivery_time(value).map(Some).ok_or(()))
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
            | "lot_watch"
            | "seller_summary"
            | "season_reminder"
    )
}

pub(crate) fn valid_status(value: &str) -> bool {
    matches!(
        value,
        "prepared"
            | "pending"
            | "claimed"
            | "sending"
            | "succeeded"
            | "failed"
            | "manual_reconcile"
    )
}

fn valid_stored_text(value: &str) -> bool {
    !value.is_empty() && value == value.trim()
}

fn valid_optional_text(value: Option<&str>) -> bool {
    value.is_none_or(valid_stored_text)
}

fn valid_returned_mail_id(value: Option<&str>) -> bool {
    value.is_none_or(|value| {
        (1..=32).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn valid_amount(value: Option<i64>) -> bool {
    value.is_none_or(|value| (0..=MAX_JSON_SAFE_INTEGER).contains(&value))
}

fn is_zero_delivery_time(value: DateTime<Utc>) -> bool {
    value.timestamp() == -62_135_596_800 && value.timestamp_subsec_nanos() == 0
}

fn valid_timeline(
    created_at: DateTime<Utc>,
    claimed_at: Option<DateTime<Utc>>,
    sending_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
) -> bool {
    claimed_at.is_none_or(|value| value >= created_at)
        && sending_at.is_none_or(|value| claimed_at.is_some_and(|claimed| value >= claimed))
        && completed_at.is_none_or(|value| sending_at.is_some_and(|sending| value >= sending))
}

fn valid_state_fields(
    value: &AdminDeliveryResponse,
    lease_expires_at: Option<DateTime<Utc>>,
    claimed_at: Option<DateTime<Utc>>,
    sending_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
) -> bool {
    // 服务端已验证未写入 metadata；浏览器只接受明确非法载荷的完整领取/完成形状。
    if value.status == "failed"
        && value.failure_code.as_deref() == Some("invalid_payload")
        && sending_at.is_none()
    {
        return value.reason_code.is_none()
            && lease_expires_at.is_none()
            && claimed_at.is_some()
            && completed_at.is_some()
            && value.external_reference.is_none()
            && !value.take_required
            && value.returned_mail_id.is_none()
            && value.fencing_version > 0
            && value.attempt_count > 0;
    }
    // watch 失效或收件人改名只能作为未发送的明确失败；不能放宽普通失败或未知发送的时间链。
    if matches!(value.reason_code.as_deref(), Some("lot_watch_invalidated" | "recipient_renamed")) {
        return value.origin_kind == "lot_watch"
            && value.status == "failed"
            && value.failure_code.as_deref() == value.reason_code.as_deref()
            && value.cod == Some(0)
            && lease_expires_at.is_none()
            && sending_at.is_none()
            && completed_at.is_some()
            && value.external_reference.is_none()
            && !value.take_required
            && value.returned_mail_id.is_none()
            && value.fencing_version > 0;
    }
    // 资产处置冲突只能在发送前隔离；人工失败后仍保留原因，不能混入普通可重试失败。
    if value.reason_code.as_deref() == Some("mail_disposition_conflict") {
        return value.origin_kind == "mail_intake"
            && match value.status.as_str() {
                "manual_reconcile" => {
                    value.failure_code.as_deref() == Some("mail_disposition_conflict")
                }
                "failed" => value.failure_code.as_deref().is_some_and(valid_stored_text),
                _ => false,
            }
            && lease_expires_at.is_none()
            && sending_at.is_none()
            && completed_at.is_some()
            && value.external_reference.is_none()
            && !value.take_required
            && value.returned_mail_id.is_none()
            && value.fencing_version > 0;
    }
    match value.status.as_str() {
        "prepared" | "pending" => {
            lease_expires_at.is_none()
                && claimed_at.is_none()
                && sending_at.is_none()
                && completed_at.is_none()
                && value.external_reference.is_none()
                && value.failure_code.is_none()
        }
        "claimed" => {
            lease_expires_at.is_some()
                && claimed_at.is_some()
                && sending_at.is_none()
                && completed_at.is_none()
                && value.external_reference.is_none()
                && value.failure_code.is_none()
                && value.fencing_version > 0
                && value.attempt_count > 0
        }
        "sending" => {
            lease_expires_at.is_some()
                && claimed_at.is_some()
                && sending_at.is_some()
                && completed_at.is_none()
                && value.external_reference.is_none()
                && value.failure_code.is_none()
                && value.fencing_version > 0
                && value.attempt_count > 0
        }
        "succeeded" => {
            lease_expires_at.is_none()
                && claimed_at.is_some()
                && sending_at.is_some()
                && completed_at.is_some()
                && value.external_reference.as_deref().is_some_and(valid_stored_text)
                && value.failure_code.is_none()
                && value.fencing_version > 0
                && value.attempt_count > 0
        }
        "failed" | "manual_reconcile" => {
            let before_activation = value.status == "failed"
                && value.origin_kind == "operations_alert"
                && value.reason_code.as_deref() == Some("operations_alert_batch_terminal")
                && lease_expires_at.is_none()
                && claimed_at.is_none()
                && sending_at.is_none()
                && completed_at.is_some()
                && value.external_reference.is_none()
                && !value.take_required
                && value.returned_mail_id.is_none()
                && matches!(
                    value.failure_code.as_deref(),
                    Some("dependency_mismatch" | "retry_exhausted")
                )
                && value.fencing_version == 0
                && value.attempt_count == 0;
            // 已证明收件人不存在的退信 MID 是人工取回线索，不是发送成功引用。
            let returned_to_sender = value.status == "manual_reconcile"
                && value.failure_code.as_deref() == Some("recipient_not_found")
                && value.reason_code.as_deref() == Some("recipient_not_found")
                && value.cod.is_some()
                && value.take_required
                && value.returned_mail_id.as_deref().is_some_and(|mid| {
                    value.external_reference.as_deref() == Some(mid)
                        && valid_returned_mail_id(Some(mid))
                });
            before_activation
                || (lease_expires_at.is_none()
                    && claimed_at.is_some()
                    && sending_at.is_some()
                    && completed_at.is_some()
                    && (value.external_reference.is_none() || returned_to_sender)
                    && value.failure_code.as_deref().is_some_and(valid_stored_text)
                    && value.fencing_version > 0
                    && value.attempt_count > 0)
        }
        _ => false,
    }
}

#[cfg(test)]
mod origin_kind_tests {
    use super::valid_origin_kind;

    /// W14.3 的两种冻结正文通知也是合法来源；闭集缺项会使投递列表整页失败关闭。
    #[test]
    fn delivery_origin_closed_set_covers_player_notifications() {
        for kind in [
            "settlement",
            "settlement_recovery_action",
            "user_key",
            "manual",
            "mail_intake",
            "operations_alert",
            "lot_watch",
            "seller_summary",
            "season_reminder",
        ] {
            assert!(valid_origin_kind(kind), "{kind} 必须被接受");
        }
        for kind in ["", "seller_summary_x", "SELLER_SUMMARY"] {
            assert!(!valid_origin_kind(kind), "{kind} 不是合法来源");
        }
    }
}
