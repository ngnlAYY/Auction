//! Admin 投递详情使用当前 fence 上的准入事实，提交仍在原事务内锁验。

use auction_application::delivery::{
    DeliveryAdminAction, DeliveryAdminGuardBlocker, DeliveryAdminGuardSnapshot,
    DeliveryAdminSnapshot,
};
use auction_domain::{DeliveryAcknowledgementFacts, validate_acknowledgement};
use serde_json::Value;
use sqlx::Postgres;
use uuid::Uuid;

use crate::database::TrackedTransaction as Transaction;
use crate::database::delivery::scheduler::recipient::{binding_matches, key_reply_user};
use crate::database::mail::delivery::disposition::mail_handed_over;
use crate::database::recovery::commands::reset_after_delivery_retry;
use crate::database::settlement::fulfillment::{
    recovery_allows_execution_now, recovery_owns_return_sql,
};

pub(super) async fn key_recipient_renamed(
    transaction: &mut Transaction<'_, Postgres>,
    delivery: &DeliveryAdminSnapshot,
    payload: &Value,
    lock: bool,
) -> Result<bool, sqlx::Error> {
    let Some(user_id) = key_reply_user(&delivery.origin_kind, payload) else {
        return Ok(false);
    };
    Ok(!binding_matches(transaction, user_id, &delivery.recipient, lock).await?)
}

pub(super) async fn permanently_retired(
    transaction: &mut Transaction<'_, Postgres>,
    delivery: &DeliveryAdminSnapshot,
) -> Result<bool, sqlx::Error> {
    // 已验证的预检退役是持久结论；旧名重新出现也不能恢复冻结收件人的旧投递。
    if delivery.retirement_reason.is_some() {
        return Ok(true);
    }
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM public.settlement_legacy_self_reconciliations WHERE delivery_id=$1)
          OR EXISTS(SELECT 1 FROM public.settlement_equipment_losses WHERE delivery_id=$1)",
    )
    .bind(delivery.id)
    .fetch_one(&mut **transaction)
    .await
}

pub(super) async fn settlement_fulfillment_state(
    transaction: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
    lock: bool,
) -> Result<Option<String>, sqlx::Error> {
    // 命令持有投递锁后才锁履约，和履约、退件及恢复写入方使用同一锁序。
    let query = if lock {
        "SELECT execution_state FROM public.settlement_obligation_fulfillments WHERE delivery_id = $1 FOR UPDATE"
    } else {
        "SELECT execution_state FROM public.settlement_obligation_fulfillments WHERE delivery_id = $1"
    };
    sqlx::query_scalar(query).bind(delivery_id).fetch_optional(&mut **transaction).await
}

pub(super) async fn read_guard(
    transaction: &mut Transaction<'_, Postgres>,
    delivery: &DeliveryAdminSnapshot,
    payload: &Value,
    result_metadata: &Value,
    command_snapshot_readable: bool,
) -> Result<(DeliveryAdminGuardSnapshot, Option<DeliveryAdminGuardBlocker>), sqlx::Error> {
    let (funded_delivery, disposed) = sqlx::query_as(
        r#"SELECT EXISTS(SELECT 1 FROM public.settlement_funding_allocations WHERE delivery_id=$1)
                  OR EXISTS(SELECT 1 FROM public.settlement_funding_delivery_bindings WHERE delivery_id=$1)
                  OR EXISTS(SELECT 1 FROM public.settlement_funding_returns WHERE delivery_id=$1),
                  EXISTS(SELECT 1 FROM public.delivery_dispositions
                         WHERE delivery_id=$1 AND fencing_version=$2 AND resolves_alert)"#,
    )
    .bind(delivery.id)
    .bind(delivery.fencing_version)
    .fetch_one(&mut **transaction)
    .await?;
    let fulfillment_state = settlement_fulfillment_state(transaction, delivery.id, false).await?;
    let acknowledge = acknowledge_blocker(delivery, payload, disposed, fulfillment_state.is_some());
    let guard = DeliveryAdminGuardSnapshot {
        action: DeliveryAdminAction::Release,
        expected_fencing_version: delivery.fencing_version,
        fencing_version: delivery.fencing_version,
        status: delivery.status.clone(),
        reason_code: delivery.reason_code.clone(),
        has_sending_fence: delivery.sending_at.is_some(),
        origin_kind: delivery.origin_kind.clone(),
        payload_operation: payload.get("operation").and_then(Value::as_str).map(str::to_owned),
        sent_reference_sighted: result_metadata.get("sent_reference_sighting").is_some(),
        funded_delivery,
        command_snapshot_readable,
        retired: permanently_retired(transaction, delivery).await?,
        lot_eligible: Some(
            crate::database::lot_watch::delivery_eligible(transaction, delivery.id).await?,
        ),
        settlement_eligible: settlement_eligible(transaction, delivery).await?,
        settlement_fulfillment_state: fulfillment_state,
        has_reconcile_reference: false,
        mail_handed_over: mail_handed_over(transaction, delivery.id).await?,
        recipient_renamed: key_recipient_renamed(transaction, delivery, payload, false).await?,
        recovery_retry_eligible: reset_after_delivery_retry(transaction, delivery.id)
            .await?
            .eligible(),
    };
    Ok((guard, acknowledge))
}

async fn settlement_eligible(
    transaction: &mut Transaction<'_, Postgres>,
    delivery: &DeliveryAdminSnapshot,
) -> Result<Option<bool>, sqlx::Error> {
    if delivery.origin_kind != "settlement" {
        return Ok(None);
    }
    let Some(obligation_id) = Uuid::parse_str(&delivery.origin_id)
        .ok()
        .filter(|id| !id.is_nil() && id.hyphenated().to_string() == delivery.origin_id)
    else {
        return Ok(Some(false));
    };
    if !recovery_allows_execution_now(transaction, obligation_id).await? {
        return Ok(Some(false));
    }
    let current: Option<Option<Uuid>> = sqlx::query_scalar(
        "SELECT delivery_id FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1",
    )
    .bind(obligation_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let recovery_owns_return: bool =
        sqlx::query_scalar(concat!("SELECT ", recovery_owns_return_sql!("$1")))
            .bind(obligation_id)
            .fetch_one(&mut **transaction)
            .await?;
    Ok(Some(current.is_none_or(|current| current == Some(delivery.id)) && !recovery_owns_return))
}

fn acknowledge_blocker(
    delivery: &DeliveryAdminSnapshot,
    payload: &Value,
    disposed: bool,
    has_fulfillment_obligation: bool,
) -> Option<DeliveryAdminGuardBlocker> {
    if delivery.fencing_version <= 0 {
        return Some(DeliveryAdminGuardBlocker::StaleFencingVersion);
    }
    if disposed {
        return Some(DeliveryAdminGuardBlocker::AlreadyDisposed);
    }
    let equipment_count = match payload.get("attachment_ids") {
        None => 0,
        Some(Value::Array(attachments)) => attachments.len(),
        Some(_) => return Some(DeliveryAdminGuardBlocker::NotAcknowledgeable),
    };
    validate_acknowledgement(DeliveryAcknowledgementFacts {
        status: &delivery.status,
        origin_kind: &delivery.origin_kind,
        credits: amount(payload.get("credits")),
        cod: amount(payload.get("cod")),
        equipment_count,
        has_fulfillment_obligation,
    })
    .err()
    .map(|_| DeliveryAdminGuardBlocker::NotAcknowledgeable)
}

// 与处置命令兼容历史的整数字符串，缺字段时沿用零资产语义。
fn amount(value: Option<&Value>) -> Option<i64> {
    let Some(value) = value else {
        return Some(0);
    };
    value.as_i64().or_else(|| {
        let text = value.as_str()?;
        let digits = text.trim_prefix('-');
        if (1..=18).contains(&digits.len()) && digits.bytes().all(|byte| byte.is_ascii_digit()) {
            text.parse().ok()
        } else {
            None
        }
    })
}
