//! 投递安全投影的 fail-closed 解码与状态机字段校验。
//! 模板/类别/世界/原因只接受服务端闭集，未知形状、越界数字与不自洽的时间链一律让整页
//! 失败关闭；浏览器不推导状态迁移，也不放宽服务端已判定的终态。

use auction_contracts::{
    AdminDeliveryCommandResponse, AdminDeliveryListResponse, AdminDeliveryResponse,
};
use auction_time::Timestamp;
use serde::Deserialize;

use super::queue::DeliveryAction;
use crate::api::pagination::valid_page_echo;
use crate::api::{
    Delivery, DeliveryPage, MAX_JSON_SAFE_INTEGER, valid_count, valid_optional_text,
    valid_success_code, valid_trimmed_text, valid_uuid,
};

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
const SUPPRESSION_REASONS: &[&str] = &[
    "mail_disposition_conflict",
    "invalid_payload",
    "task_panic",
    "dependency_mismatch",
    "retry_exhausted",
    "operations_alert_batch_terminal",
];

pub(super) fn decode_delivery_list(
    payload: StrictDeliveryEnvelope<AdminDeliveryListResponse>,
    requested_page: i64,
    requested_size: i64,
) -> Result<DeliveryPage, ()> {
    if !valid_success_code(&payload.code) || payload.message.is_empty() {
        return Err(());
    }

    let pagination = payload.data.pagination;
    if pagination.page_size > MAX_DELIVERY_PAGE_SIZE
        || !valid_page_echo(
            requested_page,
            requested_size,
            pagination.page,
            pagination.page_size,
            pagination.total,
            payload.data.deliveries.len(),
        )
    {
        return Err(());
    }

    let deliveries =
        payload.data.deliveries.into_iter().map(decode_delivery).collect::<Result<Vec<_>, _>>()?;
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
    let retirement_reason_valid = preflight_retirement_reason_valid(&value);
    // 展示标记必须绑定到已知退役原因及其来源、失败码，避免通过普通失败分支获得退役语义。
    if value.retirement_reason.as_deref().is_some_and(|reason| {
        retirement_reason_valid != Some(true) || value.reason_code.as_deref() != Some(reason)
    }) || !valid_suppression_projection(&value)
    {
        return Err(());
    }
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
    {
        return Err(());
    }
    if !valid_uuid(&value.id) {
        return Err(());
    }
    let created_at = parse_delivery_time(&value.created_at).ok_or(())?;
    let updated_at = parse_delivery_time(&value.updated_at).ok_or(())?;
    let _lease_expires_at = parse_optional_delivery_time(value.lease_expires_at.as_ref())?;
    let claimed_at = parse_optional_delivery_time(value.claimed_at.as_ref())?;
    let sending_at = parse_optional_delivery_time(value.sending_at.as_ref())?;
    let completed_at = parse_optional_delivery_time(value.completed_at.as_ref())?;

    if !valid_origin_kind(&value.origin_kind)
        || !valid_trimmed_text(&value.origin_id)
        || !valid_trimmed_text(&value.recipient)
        || !value.cod.is_none_or(valid_count)
        || !valid_status(&value.status)
        || value.fencing_version < 0
        || value.fencing_version > MAX_JSON_SAFE_INTEGER
        || value.attempt_count > i32::MAX as u32
        || is_zero_delivery_time(created_at)
        || is_zero_delivery_time(updated_at)
        || updated_at < created_at
        || claimed_at.is_some_and(is_zero_delivery_time)
        || !valid_optional_text(value.external_reference.as_deref())
        || !valid_optional_text(value.failure_code.as_deref())
        || !valid_optional_text(value.reason_code.as_deref())
        || !valid_returned_mail_id(value.returned_mail_id.as_deref())
        || !valid_state_fields(
            &value,
            created_at,
            updated_at,
            claimed_at,
            sending_at,
            completed_at,
            retirement_reason_valid,
        )
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

pub(super) fn parse_delivery_time(value: &str) -> Option<Timestamp> {
    Timestamp::parse_from_rfc3339(value).ok()
}

fn parse_optional_delivery_time(value: Option<&String>) -> Result<Option<Timestamp>, ()> {
    value.map_or(Ok(None), |value| parse_delivery_time(value).map(Some).ok_or(()))
}

fn valid_origin_kind(value: &str) -> bool {
    matches!(
        value,
        "settlement"
            | "settlement_recovery_action"
            | "settlement_return_action"
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

fn valid_returned_mail_id(value: Option<&str>) -> bool {
    value.is_none_or(|value| {
        (1..=32).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn preparation_rejected(value: &AdminDeliveryResponse) -> bool {
    value.status == "failed"
        && matches!(value.failure_code.as_deref(), Some("invalid_payload" | "task_panic"))
        && value.sending_at.is_none()
}

fn valid_suppression_projection(value: &AdminDeliveryResponse) -> bool {
    let Some(reason) = value.suppression_reason.as_deref() else {
        return true;
    };
    // 存储投影只在围栏前失败时派生抑制原因，闭集内的失败码优先于原因码。
    value.status == "failed"
        && value.sending_at.is_none()
        && value.retirement_reason.is_none()
        && value.failure_code.as_deref().filter(|code| SUPPRESSION_REASONS.contains(code)).or_else(
            || value.reason_code.as_deref().filter(|code| SUPPRESSION_REASONS.contains(code)),
        ) == Some(reason)
}

fn recovery_superseded(value: &AdminDeliveryResponse) -> bool {
    value.status == "failed" && value.failure_code.as_deref() == Some("recovery_superseded")
}

fn recovery_quarantine(value: &AdminDeliveryResponse) -> bool {
    value.status == "manual_reconcile"
        && matches!(
            value.failure_code.as_deref(),
            Some(
                "external_recovery_sent_match"
                    | "external_recovery_payload_match"
                    | "external_recovery_unresolved_send"
            )
        )
}

/// 返回已知退役原因的来源和失败码是否相符；已知原因即使组合无效也须进入退役校验。
fn preflight_retirement_reason_valid(value: &AdminDeliveryResponse) -> Option<bool> {
    let reason = value.reason_code.as_deref()?;
    let (origin_allowed, expected_failure) = match reason {
        "lot_watch_invalidated" => (value.origin_kind == "lot_watch", reason),
        "recipient_renamed" => (
            matches!(
                value.origin_kind.as_str(),
                "lot_watch" | "seller_summary" | "season_reminder"
            ),
            reason,
        ),
        "season_boundary_passed" | "season_reminder_resolved" => {
            (value.origin_kind == "season_reminder", "season_notice_invalidated")
        }
        "auction_opened" => (value.origin_kind == "mail_intake", "auction_notice_invalidated"),
        _ => return None,
    };
    Some(origin_allowed && value.failure_code.as_deref() == Some(expected_failure))
}

fn is_zero_delivery_time(value: Timestamp) -> bool {
    value.timestamp() == -62_135_596_800 && value.timestamp_subsec_nanos() == 0
}

fn valid_timeline(
    created_at: Timestamp,
    claimed_at: Option<Timestamp>,
    sending_at: Option<Timestamp>,
    completed_at: Option<Timestamp>,
) -> bool {
    claimed_at.is_none_or(|value| value >= created_at)
        && sending_at.is_none_or(|value| claimed_at.is_some_and(|claimed| value >= claimed))
        && completed_at.is_none_or(|value| sending_at.is_some_and(|sending| value >= sending))
}

fn valid_state_fields(
    value: &AdminDeliveryResponse,
    created_at: Timestamp,
    updated_at: Timestamp,
    claimed_at: Option<Timestamp>,
    sending_at: Option<Timestamp>,
    completed_at: Option<Timestamp>,
    retirement_reason_valid: Option<bool>,
) -> bool {
    let has_lease = value.lease_expires_at.is_some();
    let valid_before_send = || {
        !has_lease
            && sending_at.is_none()
            && value.external_reference.is_none()
            && !value.take_required
            && value.returned_mail_id.is_none()
            && claimed_at
                .is_none_or(|claimed| !is_zero_delivery_time(claimed) && claimed >= created_at)
            && completed_at.is_some_and(|completed| {
                !is_zero_delivery_time(completed)
                    && completed >= created_at
                    && claimed_at.is_none_or(|claimed| completed >= claimed)
                    && completed <= updated_at
            })
    };
    let claimed_has_attempt = claimed_at.is_none() || value.attempt_count > 0;

    // 专用分支按存储层 valid_snapshot 的顺序选定，匹配后一起校验时间链和状态。
    // 准备失败须保留领取与完成事实，并优先校验这一终态以拒绝相冲突的原因码。
    if preparation_rejected(value) {
        return value.reason_code.is_none()
            && claimed_at.is_some()
            && value.fencing_version > 0
            && value.attempt_count > 0
            && valid_before_send();
    }
    // 资产处置冲突只能在发送前隔离；人工失败后仍保留原因，不能混入普通可重试失败。
    if value.reason_code.as_deref() == Some("mail_disposition_conflict") {
        return value.origin_kind == "mail_intake"
            && match value.status.as_str() {
                "manual_reconcile" => {
                    value.failure_code.as_deref() == Some("mail_disposition_conflict")
                }
                "failed" => value.failure_code.as_deref().is_some_and(valid_trimmed_text),
                _ => false,
            }
            && value.fencing_version > 0
            && claimed_has_attempt
            && valid_before_send();
    }
    // 退役须有确定的未发送事实，围栏版本推进后才能让浏览器接受该终态。
    if let Some(reason_valid) = retirement_reason_valid {
        return reason_valid
            && value.status == "failed"
            && value.cod == Some(0)
            && value.suppression_reason.is_none()
            && value.fencing_version > 0
            && claimed_has_attempt
            && valid_before_send();
    }
    // 未激活的告警从创建直接到完成，未获得发送围栏。
    if value.status == "failed"
        && value.origin_kind == "operations_alert"
        && value.reason_code.as_deref() == Some("operations_alert_batch_terminal")
    {
        return claimed_at.is_none()
            && matches!(
                value.failure_code.as_deref(),
                Some("dependency_mismatch" | "retry_exhausted")
            )
            && value.fencing_version == 0
            && value.attempt_count == 0
            && valid_before_send();
    }
    // 恢复隔离或终结可从本地直接完成，领取时间本身不能作为发送尝试的证据。
    if recovery_quarantine(value) {
        return value.fencing_version > 0
            && value.retirement_reason.is_none()
            && value.suppression_reason.is_none()
            && valid_before_send();
    }
    if recovery_superseded(value) {
        return value.origin_kind == "settlement"
            && value.reason_code.is_none()
            && value.fencing_version > 0
            && valid_before_send();
    }
    if !valid_timeline(created_at, claimed_at, sending_at, completed_at) {
        return false;
    }
    match value.status.as_str() {
        "prepared" | "pending" => {
            !has_lease
                && claimed_at.is_none()
                && sending_at.is_none()
                && completed_at.is_none()
                && value.external_reference.is_none()
                && value.failure_code.is_none()
        }
        "claimed" => {
            has_lease
                && claimed_at.is_some()
                && sending_at.is_none()
                && completed_at.is_none()
                && value.external_reference.is_none()
                && value.failure_code.is_none()
                && value.fencing_version > 0
                && value.attempt_count > 0
        }
        "sending" => {
            has_lease
                && claimed_at.is_some()
                && sending_at.is_some()
                && completed_at.is_none()
                && value.external_reference.is_none()
                && value.failure_code.is_none()
                && value.fencing_version > 0
                && value.attempt_count > 0
        }
        "succeeded" => {
            !has_lease
                && claimed_at.is_some()
                && sending_at.is_some()
                && completed_at.is_some()
                && value.external_reference.as_deref().is_some_and(valid_trimmed_text)
                && value.failure_code.is_none()
                && value.fencing_version > 0
                && value.attempt_count > 0
        }
        "failed" | "manual_reconcile" => {
            // 已证明收件人不存在的退信 MID 绑定到对应世界的回收邮箱。
            let returned_to_sender = value.status == "manual_reconcile"
                && value.failure_code.as_deref() == Some("recipient_not_found")
                && value.reason_code.as_deref() == Some("recipient_not_found")
                && value.world.is_some()
                && value.cod.is_some()
                && value.take_required
                && value.returned_mail_id.as_deref().is_some_and(|mid| {
                    value.external_reference.as_deref() == Some(mid)
                        && valid_returned_mail_id(Some(mid))
                });
            !has_lease
                && claimed_at.is_some()
                && sending_at.is_some()
                && completed_at.is_some()
                && (value.external_reference.is_none() || returned_to_sender)
                && value.failure_code.as_deref().is_some_and(valid_trimmed_text)
                && value.fencing_version > 0
                && value.attempt_count > 0
        }
        _ => false,
    }
}

#[cfg(test)]
mod origin_kind_tests {
    use super::valid_origin_kind;

    /// 两种冻结正文通知与结算退件 Take 也是合法来源；闭集缺项会使投递列表整页失败关闭。
    #[test]
    fn delivery_origin_closed_set_covers_player_notifications() {
        for kind in [
            "settlement",
            "settlement_recovery_action",
            "settlement_return_action",
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
