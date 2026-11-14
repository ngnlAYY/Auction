//! Delivery 快照结构校验：状态、租约、时间线与闭集字段。

use auction_application::delivery::{
    DELIVERY_ADMIN_RETIREMENT_REASONS, DELIVERY_ADMIN_SUPPRESSION_REASONS, DeliveryAdminSnapshot,
};
use chrono::{DateTime, Utc};

use super::evidence::{preparation_rejected, valid_returned_reference};

pub(super) fn valid_live_lease(
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
        // 退役/抑制原因只允许出现在从未开始外部写入的失败终态，且二者互斥。
        || (snapshot.retirement_reason.is_some() && snapshot.suppression_reason.is_some())
        || ((snapshot.retirement_reason.is_some() || snapshot.suppression_reason.is_some())
            && !(snapshot.status == "failed" && snapshot.sending_at.is_none()))
        || snapshot
            .retirement_reason
            .as_deref()
            .is_some_and(|value| !DELIVERY_ADMIN_RETIREMENT_REASONS.contains(&value))
        || snapshot
            .suppression_reason
            .as_deref()
            .is_some_and(|value| !DELIVERY_ADMIN_SUPPRESSION_REASONS.contains(&value))
    {
        return false;
    }

    // 新准备阶段可以确定拒绝非法载荷；没有发送事实，仍要求完整 claimed/completed 时间链。
    if preparation_rejected(snapshot) {
        return snapshot.reason_code.is_none()
            && snapshot.lease_expires_at.is_none()
            && snapshot.external_reference.is_none()
            && !snapshot.take_required
            && snapshot.returned_mail_id.is_none()
            && snapshot.fencing_version > 0
            && snapshot.attempt_count > 0
            && snapshot.claimed_at.is_some_and(|claimed| {
                !is_zero_time(claimed)
                    && claimed >= snapshot.created_at
                    && snapshot.completed_at.is_some_and(|completed| {
                        !is_zero_time(completed)
                            && completed >= claimed
                            && completed <= snapshot.updated_at
                    })
            });
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

    // 仅明确失效且从未发送允许没有 sending 时间；未知发送不能进入此例外。
    // watch 提醒因 lot 失效或收件人改名退役；玩家提醒（卖方汇总、换季提醒）只在收件人改名时
    // 允许该原因，换季提醒另有边界已过/义务完成的退役原因，见下一分支。
    if matches!(
        snapshot.reason_code.as_deref(),
        Some("lot_watch_invalidated" | "recipient_renamed")
    ) {
        let origin_allowed = snapshot.origin_kind == "lot_watch"
            || (snapshot.reason_code.as_deref() == Some("recipient_renamed")
                && matches!(snapshot.origin_kind.as_str(), "seller_summary" | "season_reminder"));
        return origin_allowed
            && snapshot.failure_code.as_deref() == snapshot.reason_code.as_deref()
            && valid_preflight_retirement(snapshot);
    }

    // 换季提醒在冻结边界已过或义务全部完成时于发送围栏前退役；失败码固定。
    if matches!(
        snapshot.reason_code.as_deref(),
        Some("season_boundary_passed" | "season_reminder_resolved")
    ) {
        return snapshot.origin_kind == "season_reminder"
            && snapshot.failure_code.as_deref() == Some("season_notice_invalidated")
            && valid_preflight_retirement(snapshot);
    }

    // 投稿回执在原场次开拍后、任何发送之前退役；与 watch 一样允许只有 created/completed。
    if snapshot.reason_code.as_deref() == Some("auction_opened") {
        return snapshot.origin_kind == "mail_intake"
            && snapshot.status == "failed"
            && snapshot.failure_code.as_deref() == Some("auction_notice_invalidated")
            && snapshot.cod == Some(0)
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

    // 恢复隔离/阻塞发生在本地，可能从从未 claim 的 pending 行直接进入
    // `manual_reconcile`：没有 sending 事实，也没有 attempt；冻结/重建证据的失败码是闭集。
    // 这些行不是外部写入结果，不能套用“completed 必须有 sending”的发送时间链。
    if snapshot.status == "manual_reconcile"
        && matches!(
            snapshot.failure_code.as_deref(),
            Some(
                "external_recovery_sent_match"
                    | "external_recovery_payload_match"
                    | "external_recovery_unresolved_send"
            )
        )
    {
        return snapshot.lease_expires_at.is_none()
            && snapshot.claimed_at.is_none_or(|claimed| {
                !is_zero_time(claimed)
                    && claimed >= snapshot.created_at
                    && snapshot.completed_at.is_some_and(|completed| completed >= claimed)
            })
            && snapshot.sending_at.is_none()
            && snapshot.completed_at.is_some_and(|completed| {
                !is_zero_time(completed)
                    && completed >= snapshot.created_at
                    && completed <= snapshot.updated_at
            })
            && snapshot.external_reference.is_none()
            && !snapshot.take_required
            && snapshot.returned_mail_id.is_none()
            && snapshot.fencing_version > 0
            && snapshot.retirement_reason.is_none()
            && snapshot.suppression_reason.is_none();
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
        // 与旧版 ValidStateSnapshot 一致：prepared/pending 可保留非负 fencing/attempt 计数，
        // 但不携带执行中或已完成状态的字段。
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
                && (snapshot.external_reference.is_none() || valid_returned_reference(snapshot))
                && snapshot.failure_code.as_deref().is_some_and(valid_stored_text)
                && snapshot.fencing_version > 0
                && snapshot.attempt_count > 0
        }
        _ => false,
    }
}

/// 预检退役的公共形状：确定未写入外部系统，只有 created（可选 claimed）与 completed 事实，
/// 退役只推进 fencing version 而不产生 attempt 增加；调用方各自约束来源与失败码。
fn valid_preflight_retirement(snapshot: &DeliveryAdminSnapshot) -> bool {
    snapshot.status == "failed"
        && snapshot.cod == Some(0)
        && snapshot.lease_expires_at.is_none()
        && snapshot.sending_at.is_none()
        && snapshot.external_reference.is_none()
        && !snapshot.take_required
        && snapshot.returned_mail_id.is_none()
        && snapshot.fencing_version > 0
        && snapshot.claimed_at.is_none_or(|claimed| {
            !is_zero_time(claimed) && claimed >= snapshot.created_at && snapshot.attempt_count > 0
        })
        && snapshot.completed_at.is_some_and(|completed| {
            !is_zero_time(completed)
                && completed >= snapshot.created_at
                && snapshot.claimed_at.is_none_or(|claimed| completed >= claimed)
                && completed <= snapshot.updated_at
        })
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
            | "lot_watch"
            | "seller_summary"
            | "season_reminder"
    )
}

pub(super) fn valid_stored_text(value: &str) -> bool {
    !value.is_empty() && value == value.trim()
}

pub(super) fn is_zero_time(value: DateTime<Utc>) -> bool {
    value.timestamp() == -62_135_596_800 && value.timestamp_subsec_nanos() == 0
}
