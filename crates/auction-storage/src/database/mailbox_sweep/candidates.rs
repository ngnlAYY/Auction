//! 未证明 MoogleMail 发送的只读投影：冻结尝试身份优先，旧行只保留安全恢复所需字段。

use auction_application::{
    DeliverySendExpectation, DeliveryTakeExpectation, MoogleMailboxSweepCandidate,
    MoogleMailboxUnprovenSend,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::Postgres;
use uuid::Uuid;

use crate::database::TrackedPool;
use crate::database::storage_error_from;

/// 每次尝试的冻结身份优先；旧行只保留既有安全恢复所需的最小投影。
#[derive(sqlx::FromRow)]
pub(super) struct UnprovenSendRow {
    delivery_id: Uuid,
    origin_kind: String,
    recipient: String,
    cod: i64,
    credits: i64,
    reference: String,
    template: String,
    result_code: Option<String>,
    user_key_user_id: Option<Uuid>,
    inbound_mail_id: String,
    sending_at: DateTime<Utc>,
    outcome_recorded_at: DateTime<Utc>,
    operation: String,
    attachment_value: Value,
    sweep_retries: i64,
    fencing_version: i64,
    sending_fencing_version: i64,
    has_send_attempt: bool,
    expectation: Option<Value>,
    has_take_attempt: bool,
    take_expectation: Option<Value>,
}

pub(super) async fn unproven_sends(
    pool: &TrackedPool<Postgres>,
    world_id: &str,
    limit: i64,
) -> Result<Vec<MoogleMailboxSweepCandidate>, auction_application::ApplicationError> {
    // 所有 Send 来源共享冻结身份；不按 origin 白名单漏掉 watch、alert 或恢复发送。
    // 仅旧结算解析既有装备引用；正文和登录 key 不进入维护投影。
    // 先物化轮转前缀，再解析冻结尝试和旧装备引用，避免为本批未处理的后缀执行关联查询。
    let rows: Vec<UnprovenSendRow> = sqlx::query_as(
        "WITH candidates AS MATERIALIZED (
            SELECT d.id, d.created_at,
                   CASE WHEN sweep.last_created_at IS NULL
                              OR (d.created_at, d.id) > (sweep.last_created_at, sweep.last_delivery_id)
                        THEN 0 ELSE 1 END AS sweep_order
            FROM public.deliveries d
            LEFT JOIN public.mailbox_sweep_cursors sweep ON sweep.world_id = $1
            WHERE d.status = 'manual_reconcile'
              AND d.payload_operation IN ('send', 'take')
              AND d.external_reference IS NULL
              AND d.sending_at IS NOT NULL
              AND d.completed_at IS NOT NULL
              AND COALESCE(d.payload_world, 'main') = $1
            ORDER BY sweep_order, d.created_at, d.id
            LIMIT $2
         )
         SELECT d.id AS delivery_id,
                d.origin_kind,
                d.recipient,
                COALESCE((d.payload->>'cod')::bigint, 0) AS cod,
                COALESCE((d.payload->>'credits')::bigint, 0) AS credits,
                COALESCE(d.payload->'variables'->>'reference', '') AS reference,
                COALESCE(d.payload_template, '') AS template,
                d.payload->>'result_code' AS result_code,
                NULLIF(d.payload->>'user_key_user_id', '')::uuid AS user_key_user_id,
                COALESCE(d.payload->>'external_mail_id', '') AS inbound_mail_id,
                d.sending_at,
                d.completed_at AS outcome_recorded_at,
                COALESCE(d.payload_operation, '') AS operation,
                CASE WHEN attempt.expectation IS NOT NULL
                    THEN attempt.expectation->'equipment_ids'
                WHEN d.origin_kind='settlement' THEN (
                    SELECT CASE
                        WHEN count(e.id)=jsonb_array_length(
                            COALESCE(d.payload->'equipment_secret_refs','[]'::jsonb))
                        THEN COALESCE(jsonb_agg(e.external_equipment_id ORDER BY e.external_equipment_id)
                            FILTER (WHERE e.id IS NOT NULL),'[]'::jsonb)
                        ELSE 'null'::jsonb END
                    FROM jsonb_array_elements(
                        COALESCE(d.payload->'equipment_secret_refs','[]'::jsonb)) AS ref
                    LEFT JOIN public.equipment e ON e.id::text=ref->>'id'
                        AND ref->>'kind'='auction_equipment_key'
                        AND e.world_id=COALESCE(d.payload_world,'main')
                ) ELSE COALESCE(d.payload->'attachment_ids','[]'::jsonb) END AS attachment_value,
                COALESCE((d.result_metadata->>'sweep_retries')::bigint, 0) AS sweep_retries,
                d.fencing_version,
                COALESCE(attempt.sending_fencing_version,take_attempt.taking_fencing_version,
                         d.fencing_version) AS sending_fencing_version,
                EXISTS (SELECT 1 FROM public.delivery_send_attempts history
                        WHERE history.delivery_id=d.id) AS has_send_attempt,
                attempt.expectation,
                EXISTS (SELECT 1 FROM public.delivery_take_attempts history
                        WHERE history.delivery_id=d.id) AS has_take_attempt,
                take_attempt.expectation AS take_expectation
         FROM candidates
         JOIN public.deliveries d ON d.id = candidates.id
         LEFT JOIN LATERAL public.delivery_current_send_attempt(d.id) attempt ON true
         LEFT JOIN LATERAL public.delivery_current_take_attempt(d.id) take_attempt ON true
         ORDER BY candidates.sweep_order, candidates.created_at, candidates.id",
    )
    .bind(world_id)
    .bind(limit)
    .fetch_all(pool.raw())
    .await
    .map_err(|error| storage_error_from(error, "MoogleMail 未证明投递查询失败"))?;
    // 不可重建行仍返回轮转身份，避免固定坏前缀永久隐藏后缀。
    Ok(rows
        .into_iter()
        .map(|row| MoogleMailboxSweepCandidate {
            delivery_id: row.delivery_id,
            send: recoverable_send(world_id, row),
        })
        .collect())
}

fn recoverable_send(world_id: &str, row: UnprovenSendRow) -> Option<MoogleMailboxUnprovenSend> {
    let UnprovenSendRow {
        delivery_id,
        origin_kind,
        recipient,
        cod,
        credits,
        reference,
        template,
        result_code,
        user_key_user_id,
        inbound_mail_id,
        sending_at,
        outcome_recorded_at,
        operation,
        attachment_value,
        sweep_retries,
        fencing_version,
        sending_fencing_version,
        has_send_attempt,
        expectation,
        has_take_attempt,
        take_expectation,
    } = row;
    if recipient.is_empty() || cod < 0 || credits < 0 || outcome_recorded_at < sending_at {
        return None;
    }
    if (has_send_attempt && expectation.is_none())
        || (has_take_attempt && take_expectation.is_none())
    {
        // 当前发送签名损坏或已换尝试，不能把新格式记录降级成旧模板恢复。
        return None;
    }
    let attachment_ids = serde_json::from_value::<Vec<String>>(attachment_value).ok()?;
    let send_expectation = match expectation {
        Some(value) => {
            let frozen: DeliverySendExpectation = serde_json::from_value(value).ok()?;
            if !frozen.validate()
                || frozen.world_id != world_id
                || frozen.recipient != recipient
                || frozen.cod != cod
                || frozen.credits != credits
                || frozen.equipment_ids != attachment_ids
                || operation != "send"
            {
                return None;
            }
            Some(frozen)
        }
        None => None,
    };
    let take_expectation = match take_expectation {
        Some(value) => {
            let frozen: DeliveryTakeExpectation = serde_json::from_value(value).ok()?;
            if !frozen.validate()
                || frozen.world_id != world_id
                || frozen.external_mail_id != inbound_mail_id
                || operation != "take"
            {
                return None;
            }
            Some(frozen)
        }
        None => None,
    };
    let usable = send_expectation.is_some()
        || match (origin_kind.as_str(), operation.as_str()) {
            ("settlement", "send") => !reference.is_empty(),
            ("mail_intake", "send") => result_code.is_some() && !inbound_mail_id.is_empty(),
            ("mail_intake", "take") => {
                !inbound_mail_id.is_empty()
                    && !attachment_ids.is_empty()
                    && attachment_ids.iter().all(|id| {
                        !id.is_empty()
                            && id.len() <= 32
                            && id.bytes().all(|byte| byte.is_ascii_digit())
                    })
            }
            _ => false,
        };
    if !usable {
        return None;
    }
    Some(MoogleMailboxUnprovenSend {
        delivery_id,
        origin_kind,
        world_id: world_id.to_owned(),
        recipient,
        cod,
        credits,
        inbound_mail_id,
        result_code,
        user_key_user_id,
        sending_at,
        outcome_recorded_at,
        operation,
        attachment_ids,
        sweep_retries,
        reference,
        template,
        fencing_version,
        sending_fencing_version,
        send_expectation,
        take_expectation,
    })
}
