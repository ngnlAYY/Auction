//! PostgreSQL delivery Admin 详情读取。
//!
//! 一次 REPEATABLE READ 只读快照同时读取投递行、追加式尝试、命令账本与来源义务，
//! 避免 Worker 推进状态时把不同时刻的证据拼成一条详情。所有字段都是安全投影：
//! 不返回 payload、request digest、lease token、Cookie 或登录 key 正文。

use std::collections::HashSet;

use auction_application::delivery::{
    DeliveryAdminAttempt, DeliveryAdminCustody, DeliveryAdminDetail, DeliveryAdminDisposition,
    DeliveryAdminError, DeliveryAdminObligation, DeliveryAdminRecovery, DeliveryAdminSentEvidence,
    DeliverySendExpectation, DeliveryTakeExpectation,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::map_storage_error;
use super::mapping::{decode_result_payload, snapshot};
use crate::database::{TrackedPool, TrackedTransaction as Transaction};

pub(crate) async fn detail(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
) -> Result<DeliveryAdminDetail, DeliveryAdminError> {
    let mut transaction = pool.begin_read_snapshot().await.map_err(map_storage_error)?;
    let row = sqlx::query(
        r#"SELECT id, origin_kind, origin_id, recipient, payload, result_metadata, status,
                  lease_token, lease_expires_at, fencing_version, attempt_count, claimed_at,
                  sending_at, completed_at, external_reference, failure_code,
                  created_at, updated_at
           FROM public.deliveries WHERE id = $1"#,
    )
    .bind(delivery_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_storage_error)?
    .ok_or(DeliveryAdminError::NotFound)?;
    let result_metadata: Value = row.try_get("result_metadata").map_err(map_storage_error)?;
    let delivery = snapshot(&row)?;
    let attempts = attempts(&mut transaction, delivery_id).await?;
    let confirmation_rows = confirmations(&mut transaction, delivery_id).await?;
    let mut dispositions = admin_commands(&mut transaction, delivery_id).await?;
    let custody = custody(&result_metadata, &confirmation_rows);
    dispositions.extend(confirmation_rows.iter().map(confirmation_disposition));
    let mut seen: HashSet<Uuid> = dispositions.iter().map(|item| item.command_id).collect();
    dispositions.extend(
        unified_dispositions(&mut transaction, delivery_id)
            .await?
            .into_iter()
            .filter(|item| seen.insert(item.command_id)),
    );
    let evidence = evidence(&result_metadata, &confirmation_rows);
    let (obligation, recovery) = origin_chain(&mut transaction, delivery_id).await?;
    transaction.commit().await.map_err(map_storage_error)?;
    dispositions.sort_by(|left, right| {
        right.occurred_at.cmp(&left.occurred_at).then(right.command_id.cmp(&left.command_id))
    });
    Ok(DeliveryAdminDetail {
        delivery,
        attempts,
        evidence,
        custody,
        dispositions,
        obligation,
        recovery,
    })
}

/// 统一处置表的安全投影。部分种类没有命令账本身份（自动 Sent 证明、预检退役），
/// 此时以处置事实自身的 UUID 作为稳定展示身份；它不参与业务判断。
async fn unified_dispositions(
    transaction: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
) -> Result<Vec<DeliveryAdminDisposition>, DeliveryAdminError> {
    let rows = sqlx::query(
        r#"SELECT id, fencing_version, kind, command_id, admin_command_id, equipment_loss_id,
                  legacy_self_reconciliation_id, sent_confirmation_id, settlement_recovery_id,
                  source_audit_id, actor_id, reason, reason_code, occurred_at
           FROM public.delivery_dispositions
           WHERE delivery_id = $1
           ORDER BY occurred_at DESC, id DESC"#,
    )
    .bind(delivery_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    rows.iter()
        .map(|row| {
            let id: Uuid = row.try_get("id").map_err(map_storage_error)?;
            let identity = row
                .try_get::<Option<Uuid>, _>("command_id")
                .map_err(map_storage_error)?
                .or(row.try_get("admin_command_id").map_err(map_storage_error)?)
                .or(row.try_get("equipment_loss_id").map_err(map_storage_error)?)
                .or(row.try_get("legacy_self_reconciliation_id").map_err(map_storage_error)?)
                .or(row.try_get("sent_confirmation_id").map_err(map_storage_error)?)
                .or(row.try_get("settlement_recovery_id").map_err(map_storage_error)?)
                .or(row.try_get("source_audit_id").map_err(map_storage_error)?)
                .unwrap_or(id);
            Ok(DeliveryAdminDisposition {
                command_id: identity,
                kind: row.try_get("kind").map_err(map_storage_error)?,
                occurred_at: row.try_get("occurred_at").map_err(map_storage_error)?,
                status: None,
                fencing_version: Some(row.try_get("fencing_version").map_err(map_storage_error)?),
                failure_code: row.try_get("reason_code").map_err(map_storage_error)?,
                external_reference: None,
                actor: Some(row.try_get("actor_id").map_err(map_storage_error)?),
                reason: row.try_get("reason").map_err(map_storage_error)?,
            })
        })
        .collect()
}

async fn attempts(
    transaction: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
) -> Result<Vec<DeliveryAdminAttempt>, DeliveryAdminError> {
    let rows = sqlx::query(
        r#"SELECT 'send'::text AS operation, attempt.fencing_version, attempt.world_id,
                  attempt.sending_at, attempt.expectation,
                  binding.external_mail_id AS bound_sent_mail_id
           FROM public.delivery_send_attempts attempt
           LEFT JOIN public.delivery_sent_bindings binding
             ON binding.delivery_id = attempt.delivery_id
            AND binding.fencing_version = attempt.fencing_version
           WHERE attempt.delivery_id = $1
           UNION ALL
           SELECT 'take'::text, attempt.fencing_version, attempt.world_id,
                  attempt.sending_at, attempt.expectation, NULL::text
           FROM public.delivery_take_attempts attempt
           WHERE attempt.delivery_id = $1
           ORDER BY sending_at DESC, fencing_version DESC, operation"#,
    )
    .bind(delivery_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    rows.iter()
        .map(|row| {
            let operation: String = row.try_get("operation").map_err(map_storage_error)?;
            let expectation: Value = row.try_get("expectation").map_err(map_storage_error)?;
            let mut attempt = DeliveryAdminAttempt {
                fencing_version: row.try_get("fencing_version").map_err(map_storage_error)?,
                world_id: row.try_get("world_id").map_err(map_storage_error)?,
                sending_at: row.try_get("sending_at").map_err(map_storage_error)?,
                recipient: None,
                subject: None,
                body_digest: None,
                credits: None,
                cod: None,
                equipment_ids: Vec::new(),
                external_mail_id: None,
                credential: None,
                bound_sent_mail_id: row.try_get("bound_sent_mail_id").map_err(map_storage_error)?,
                operation,
            };
            if attempt.operation == "send" {
                let expectation: DeliverySendExpectation = serde_json::from_value(expectation)
                    .map_err(|_: serde_json::Error| DeliveryAdminError::StoredResultCorrupt)?;
                attempt.recipient = Some(expectation.recipient);
                attempt.subject = Some(expectation.subject);
                attempt.body_digest = Some(expectation.body_digest);
                attempt.credits = Some(expectation.credits);
                attempt.cod = Some(expectation.cod);
                attempt.equipment_ids = expectation.equipment_ids;
                attempt.credential = expectation.credential;
            } else {
                let expectation: DeliveryTakeExpectation = serde_json::from_value(expectation)
                    .map_err(|_: serde_json::Error| DeliveryAdminError::StoredResultCorrupt)?;
                attempt.external_mail_id = Some(expectation.external_mail_id);
                attempt.credential = expectation.credential;
            }
            Ok(attempt)
        })
        .collect()
}

struct ConfirmationRow {
    command_id: Uuid,
    actor: String,
    reason: String,
    confirmed_at: DateTime<Utc>,
    external_mail_id: String,
    receipt: String,
    collected: bool,
}

async fn confirmations(
    transaction: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
) -> Result<Vec<ConfirmationRow>, DeliveryAdminError> {
    let rows = sqlx::query(
        r#"SELECT command_id, actor, reason, evidence, confirmed_at
           FROM public.delivery_sent_confirmations
           WHERE delivery_id = $1
           ORDER BY confirmed_at, command_id"#,
    )
    .bind(delivery_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    rows.iter()
        .map(|row| {
            let evidence: Value = row.try_get("evidence").map_err(map_storage_error)?;
            let external_mail_id = evidence
                .get("external_mail_id")
                .and_then(Value::as_str)
                .ok_or(DeliveryAdminError::StoredResultCorrupt)?;
            let receipt = evidence
                .get("receipt")
                .and_then(Value::as_str)
                .ok_or(DeliveryAdminError::StoredResultCorrupt)?;
            let collected = evidence
                .get("collected")
                .and_then(Value::as_bool)
                .ok_or(DeliveryAdminError::StoredResultCorrupt)?;
            Ok(ConfirmationRow {
                command_id: row.try_get("command_id").map_err(map_storage_error)?,
                actor: row.try_get("actor").map_err(map_storage_error)?,
                reason: row.try_get("reason").map_err(map_storage_error)?,
                confirmed_at: row.try_get("confirmed_at").map_err(map_storage_error)?,
                external_mail_id: external_mail_id.to_owned(),
                receipt: receipt.to_owned(),
                collected,
            })
        })
        .collect()
}

async fn admin_commands(
    transaction: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
) -> Result<Vec<DeliveryAdminDisposition>, DeliveryAdminError> {
    let rows = sqlx::query(
        r#"SELECT command_id, command_kind, result_payload, created_at
           FROM public.delivery_admin_commands
           WHERE delivery_id = $1"#,
    )
    .bind(delivery_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    rows.iter()
        .map(|row| {
            let command_id: Uuid = row.try_get("command_id").map_err(map_storage_error)?;
            let payload: Value = row.try_get("result_payload").map_err(map_storage_error)?;
            // 复用命令账本的双读解码，不把旧完整结果或损坏结果当作新投影。
            let result = decode_result_payload(payload, command_id)?;
            Ok(DeliveryAdminDisposition {
                command_id,
                kind: row.try_get("command_kind").map_err(map_storage_error)?,
                occurred_at: row.try_get("created_at").map_err(map_storage_error)?,
                status: Some(result.snapshot.status),
                fencing_version: Some(result.snapshot.fencing_version),
                failure_code: result.snapshot.failure_code,
                external_reference: result.snapshot.external_reference,
                actor: None,
                reason: None,
            })
        })
        .collect()
}

fn confirmation_disposition(row: &ConfirmationRow) -> DeliveryAdminDisposition {
    DeliveryAdminDisposition {
        command_id: row.command_id,
        kind: "sent_operator_confirmed".to_owned(),
        occurred_at: row.confirmed_at,
        status: None,
        fencing_version: None,
        failure_code: None,
        external_reference: Some(format!("mooglemail:main:send:{}", row.external_mail_id)),
        actor: Some(row.actor.clone()),
        reason: Some(row.reason.clone()),
    }
}

/// 最近一次观测决定展示的领取状态；缺失保持 `None`，绝不推断未领取。
fn custody(metadata: &Value, confirmations: &[ConfirmationRow]) -> DeliveryAdminCustody {
    let observations = observations(metadata);
    let latest = observations.first();
    let operator_confirmed_collected = confirmations.iter().any(|row| row.collected);
    DeliveryAdminCustody {
        collected: if operator_confirmed_collected {
            Some(true)
        } else {
            latest.and_then(|item| item.collected)
        },
        cod_pending: latest.and_then(|item| item.cod_pending),
        remaining_credits: latest.and_then(|item| item.remaining_credits),
        // 回执只来自人工证明证据；自动观察没有回执，不能伪造。
        receipt: confirmations
            .iter()
            .rev()
            .find_map(|row| (!row.receipt.is_empty()).then(|| row.receipt.clone())),
        observed_at: latest.and_then(|item| item.observed_at),
        operator_confirmed_collected,
    }
}

fn evidence(metadata: &Value, confirmations: &[ConfirmationRow]) -> Vec<DeliveryAdminSentEvidence> {
    let mut items = observations(metadata);
    items.extend(confirmations.iter().map(|row| DeliveryAdminSentEvidence {
        reference: format!("sent_operator_confirmed:{}", row.external_mail_id),
        kind: "sent_operator_confirmed".to_owned(),
        external_mail_id: Some(row.external_mail_id.clone()),
        observed_at: Some(row.confirmed_at),
        // 人工证明声明的是观测责任，不提供新的发送时间或正文摘要。
        sent_at: None,
        recipient: None,
        subject: None,
        body_digest: None,
        collected: Some(row.collected),
        cod_pending: None,
        remaining_credits: None,
        remaining_equipment_count: None,
        receipt: (!row.receipt.is_empty()).then(|| row.receipt.clone()),
    }));
    items.sort_by_key(|item| std::cmp::Reverse(item.observed_at));
    items
}

/// 从 `result_metadata` 提取维护流程写入的三类观察；未知键不解释成证据。
fn observations(metadata: &Value) -> Vec<DeliveryAdminSentEvidence> {
    let mut items = Vec::new();
    for (key, kind) in [
        ("mailbox_sweep", "mailbox_sweep"),
        ("sent_reference_sighting", "sent_reference_sighting"),
        ("compose_discard", "compose_discard"),
    ] {
        let Some(observation) = metadata.get(key).filter(|value| value.is_object()) else {
            continue;
        };
        let external_mail_id = observation
            .get("mid")
            .or_else(|| observation.get("external_mail_id"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let reference = match external_mail_id.as_deref() {
            Some(mail_id) => format!("{kind}:{mail_id}"),
            None => kind.to_owned(),
        };
        items.push(DeliveryAdminSentEvidence {
            reference,
            kind: kind.to_owned(),
            external_mail_id,
            observed_at: timestamp(observation.get("observed_at")),
            sent_at: timestamp(observation.get("sent_at")),
            recipient: text(observation.get("recipient")),
            subject: text(observation.get("subject")),
            body_digest: digest(observation.get("body_digest")),
            collected: observation.get("collected").and_then(Value::as_bool),
            cod_pending: observation.get("cod_pending").and_then(Value::as_i64),
            remaining_credits: observation.get("remaining_credits").and_then(Value::as_i64),
            remaining_equipment_count: observation
                .get("remaining_equipment_count")
                .and_then(Value::as_i64)
                .or_else(|| {
                    observation
                        .get("remaining_equipment_ids")
                        .and_then(Value::as_array)
                        .map(|ids| ids.len() as i64)
                }),
            receipt: text(observation.get("receipt")),
        });
    }
    items.sort_by_key(|item| std::cmp::Reverse(item.observed_at));
    items
}

fn timestamp(value: Option<&Value>) -> Option<DateTime<Utc>> {
    value
        .and_then(Value::as_str)
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .map(|time| time.with_timezone(&Utc))
}

fn text(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

fn digest(value: Option<&Value>) -> Option<[u8; 32]> {
    let array = value?.as_array()?;
    if array.len() != 32 {
        return None;
    }
    let mut bytes = [0_u8; 32];
    for (index, byte) in array.iter().enumerate() {
        bytes[index] = u8::try_from(byte.as_u64()?).ok()?;
    }
    Some(bytes)
}

async fn origin_chain(
    transaction: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
) -> Result<(Option<DeliveryAdminObligation>, Option<DeliveryAdminRecovery>), DeliveryAdminError> {
    let row = sqlx::query(
        r#"SELECT obligation.id AS obligation_id, obligation.auction_id,
                  obligation.lot_id, obligation.world_id, obligation.obligation_kind,
                  obligation.business_state, obligation.amount,
                  fulfillment.execution_state AS fulfillment_state,
                  fulfillment.last_error_code AS fulfillment_error_code,
                  recovery.id AS recovery_id, recovery.stage, recovery.resolution,
                  recovery.payment_deadline_at, recovery.payment_obligation_id,
                  recovery.blocking_code, recovery.manual_reconcile_delivery_id
           FROM public.deliveries delivery
           LEFT JOIN LATERAL (
               SELECT candidate.*
               FROM public.settlement_obligations candidate
               WHERE (delivery.origin_kind = 'settlement' AND candidate.id::text = delivery.origin_id)
                  OR (
                      delivery.origin_kind = 'settlement_recovery_action'
                      AND candidate.id = (
                          SELECT action_recovery.payment_obligation_id
                          FROM public.settlement_recovery_actions action
                          JOIN public.settlement_recoveries action_recovery
                            ON action_recovery.id = action.recovery_id
                          WHERE action.id::text = delivery.origin_id
                      )
                  )
               LIMIT 1
           ) obligation ON true
           LEFT JOIN public.settlement_obligation_fulfillments fulfillment
             ON fulfillment.obligation_id = obligation.id
           LEFT JOIN LATERAL (
               SELECT candidate.*
               FROM public.settlement_recoveries candidate
               WHERE candidate.payment_obligation_id = obligation.id
                  OR (
                      delivery.origin_kind = 'settlement_recovery_action'
                      AND candidate.id = (
                          SELECT action.recovery_id
                          FROM public.settlement_recovery_actions action
                          WHERE action.id::text = delivery.origin_id
                      )
                  )
               LIMIT 1
           ) recovery ON true
           WHERE delivery.id = $1"#,
    )
    .bind(delivery_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    let obligation_id: Option<Uuid> = row.try_get("obligation_id").map_err(map_storage_error)?;
    let obligation = match obligation_id {
        None => None,
        Some(obligation_id) => Some(DeliveryAdminObligation {
            obligation_id,
            auction_id: row.try_get("auction_id").map_err(map_storage_error)?,
            lot_id: row.try_get("lot_id").map_err(map_storage_error)?,
            world_id: row.try_get("world_id").map_err(map_storage_error)?,
            obligation_kind: row.try_get("obligation_kind").map_err(map_storage_error)?,
            business_state: row.try_get("business_state").map_err(map_storage_error)?,
            amount: row.try_get("amount").map_err(map_storage_error)?,
            fulfillment_state: row.try_get("fulfillment_state").map_err(map_storage_error)?,
            fulfillment_error_code: row
                .try_get("fulfillment_error_code")
                .map_err(map_storage_error)?,
        }),
    };
    let recovery_id: Option<Uuid> = row.try_get("recovery_id").map_err(map_storage_error)?;
    let recovery = match recovery_id {
        None => None,
        Some(recovery_id) => Some(DeliveryAdminRecovery {
            recovery_id,
            stage: row.try_get("stage").map_err(map_storage_error)?,
            resolution: row.try_get("resolution").map_err(map_storage_error)?,
            payment_deadline_at: row.try_get("payment_deadline_at").map_err(map_storage_error)?,
            payment_obligation_id: row
                .try_get("payment_obligation_id")
                .map_err(map_storage_error)?,
            blocking_code: row.try_get("blocking_code").map_err(map_storage_error)?,
            manual_reconcile_delivery_id: row
                .try_get("manual_reconcile_delivery_id")
                .map_err(map_storage_error)?,
        }),
    };
    Ok((obligation, recovery))
}
