//! 排除同 EID/world 的全部本系统占用，关联不全失败关闭；不替旧 unknown 洗白。

use crate::database::TrackedTransaction as Transaction;
use std::{collections::HashSet, sync::LazyLock};

use super::sql_error;
use auction_application::{FundingError, SettlementFulfillmentClaim};
use auction_domain::valid_text as valid_line;
use futures_util::TryStreamExt;
use serde::Deserialize;
use sqlx::{Postgres, Row};
use uuid::Uuid;

mod intake;

struct AssetConflictTarget<'a> {
    kind: auction_domain::SettlementObligationKind,
    obligation_id: auction_domain::SettlementId,
    lot_id: auction_domain::LotId,
    external_equipment_id: &'a str,
    equipment_world: auction_domain::SettlementWorld,
}

pub(super) async fn exclude_asset_conflicts(
    tx: &mut Transaction<'_, Postgres>,
    c: &SettlementFulfillmentClaim,
) -> Result<(), FundingError> {
    let target = AssetConflictTarget {
        kind: c.kind,
        obligation_id: c.obligation_id,
        lot_id: c.lot_id,
        external_equipment_id: &c.external_equipment_id,
        equipment_world: c.equipment_world,
    };
    exclude_conflicts(tx, &target, None).await
}

/// 装备损失仅忽略已证明未外写的原投递；其余入库、恢复与出站冲突复用既有门。
pub(crate) async fn exclude_equipment_loss_conflicts(
    tx: &mut Transaction<'_, Postgres>,
    lot_id: Uuid,
    payment_id: Uuid,
    external_equipment_id: &str,
    delivery_id: Uuid,
) -> Result<(), FundingError> {
    let target = AssetConflictTarget {
        kind: auction_domain::SettlementObligationKind::WinnerEquipmentCod,
        obligation_id: auction_domain::SettlementId::from_uuid(payment_id),
        lot_id: auction_domain::LotId::from_uuid(lot_id),
        external_equipment_id,
        equipment_world: auction_domain::SettlementWorld::Main,
    };
    exclude_conflicts(tx, &target, Some(delivery_id)).await
}

async fn exclude_conflicts(
    tx: &mut Transaction<'_, Postgres>,
    c: &AssetConflictTarget<'_>,
    ignored_delivery: Option<Uuid>,
) -> Result<(), FundingError> {
    // 仅原equipment_recovered门已闭合且原handoff有明确Sent成功时排除该旧出站。
    // prepared/sending/manual_reconcile及未知结果不在此例外内，也不改写旧义务。
    let recovered_handoff: Option<uuid::Uuid> = if c.kind
        == auction_domain::SettlementObligationKind::BuyerRefund
    {
        sqlx::query_scalar("SELECT d.id FROM public.settlement_recoveries r JOIN public.settlement_obligation_fulfillments h ON h.obligation_id=r.equipment_obligation_id JOIN public.deliveries d ON d.id=h.delivery_id JOIN public.settlement_recovery_actions a ON a.recovery_id=r.id AND a.kind='recover_equipment_take' WHERE r.buyer_refund_obligation_id=$1 AND r.stage='refunding' AND r.resolution='equipment_recovered' AND r.custody_proof_digest=a.proof_digest AND a.execution_state='satisfied' AND a.proof_digest IS NOT NULL AND h.execution_state='enqueued' AND d.status='succeeded' AND (d.result_metadata->>'proof'='sent_exact_payload_v1' OR public.delivery_has_frozen_sent_proof(d.id,d.result_metadata,d.external_reference)) AND d.result_metadata->>'external_reference_proven'='true'")
            .bind(c.obligation_id.as_uuid()).fetch_optional(&mut **tx).await.map_err(sql_error)?
    } else {
        ignored_delivery
    };
    let conflict: bool = sqlx::query_scalar(
        "SELECT
         EXISTS(SELECT 1 FROM public.settlement_obligations o
           JOIN public.equipment e ON e.id=o.equipment_id
           LEFT JOIN public.settlement_obligation_fulfillments f ON f.obligation_id=o.id
           WHERE e.external_equipment_id=$1 AND e.world_id=$2 AND o.id<>$3
             AND (f.delivery_id IS NULL OR f.delivery_id IS DISTINCT FROM $5)
             AND ((o.lot_id<>$4 AND COALESCE(f.execution_state,'pending')<>'satisfied')
               OR f.execution_state IN ('prepared','enqueued','manual_reconcile')
               OR (f.execution_state='pending' AND (f.delivery_id IS NOT NULL OR f.prepare_command_id IS NOT NULL OR f.activate_command_id IS NOT NULL))))
         OR EXISTS(SELECT 1 FROM public.settlement_recoveries r
           JOIN public.settlement_obligations o ON o.id=r.payment_obligation_id
           JOIN public.equipment e ON e.id=o.equipment_id
           WHERE e.external_equipment_id=$1 AND e.world_id=$2
             AND (r.blocking_code IS NOT NULL OR r.manual_reconcile_delivery_id IS NOT NULL
               OR (r.lot_id<>$4 AND r.stage<>'completed')
               OR (r.lot_id=$4 AND r.stage IN ('reclaiming','relist_pending'))))
         OR EXISTS(SELECT 1 FROM public.settlement_recovery_actions a
           JOIN public.settlement_recoveries r ON r.id=a.recovery_id
           JOIN public.settlement_obligations o ON o.id=r.payment_obligation_id
           JOIN public.equipment e ON e.id=o.equipment_id
           WHERE e.external_equipment_id=$1 AND e.world_id=$2
             AND a.execution_state<>'satisfied')"
    ).bind(c.external_equipment_id).bind(c.equipment_world.as_str()).bind(c.obligation_id.as_uuid())
        .bind(c.lot_id.as_uuid()).bind(recovered_handoff).fetch_one(&mut **tx).await.map_err(sql_error)?;
    if conflict {
        return Err(FundingError::Unproven);
    }
    // 普通通知与入站处置用attachment_ids，而非settlement的装备secret引用；只排除已验证schema
    // 且不涉及目标EID/world的普通载荷，来源关联仍由后续SQL独立核对。论坛通知复用mail_intake，
    // 但owner是同command/world的notify账本，按producer重建origin；此处只读快照，不反取论坛锁。
    // 必须完整扫描历史候选，但正文与附件逐行释放；内存只保留闭集验证通过的 UUID。
    let mut unrelated_attachments = Vec::new();
    let mut deliveries = sqlx::query(
        "SELECT d.id, d.command_id, d.origin_kind, d.origin_id, d.payload,
           EXISTS(SELECT 1 FROM public.auction_forum_bid_commands forum_command
             WHERE forum_command.command_id=d.command_id
               AND forum_command.command_kind='notify' AND forum_command.amount=0
               AND forum_command.world_id=d.payload->>'world'
               AND d.origin_id='forum:' || forum_command.auction_id::text || ':'
                   || forum_command.thread_id || ':' || forum_command.post_id || ':'
                   || forum_command.line_number::text || ':' || forum_command.lot_code
           ) AS forum_notice_owner
         FROM public.deliveries d
         WHERE d.origin_kind IN ('mail_intake','settlement_recovery_action','operations_alert','lot_watch')",
    )
    .fetch(&mut **tx);
    while let Some(delivery) = deliveries.try_next().await.map_err(sql_error)? {
        let payload: serde_json::Value = delivery.try_get("payload").map_err(sql_error)?;
        let origin: &str = delivery.try_get("origin_kind").map_err(sql_error)?;
        let origin_id: &str = delivery.try_get("origin_id").map_err(sql_error)?;
        let command_id: Uuid = delivery.try_get("command_id").map_err(sql_error)?;
        let forum_notice_owner: bool = delivery.try_get("forum_notice_owner").map_err(sql_error)?;
        if unrelated_attachment_payload(
            &payload,
            origin,
            origin_id,
            command_id,
            forum_notice_owner,
            c,
        ) {
            unrelated_attachments.push(delivery.try_get::<Uuid, _>("id").map_err(sql_error)?);
        }
    }
    drop(deliveries);
    // 当前标的的最终入库证明只闭合精确绑定的 take 及更早已完成 return；
    // 未知、后续动作或其他标的历史仍进入原有冲突门，不能仅凭 succeeded 放行。
    unrelated_attachments.extend(intake::closed_source_history(tx, c).await?);
    // JSON只用于定位出站资产，不是资金授权。schema损坏/未知世界无法排除关联，保守阻断。
    // 成功delivery必须有对应的已满足fulfillment或recovery action；孤儿成功也不是清白。
    // 竞得装备正文只接受精确旧三/八变量或显式 v2 九变量；缺字段、未知模式/字段和非字符串值仍视为冲突。
    // 只缓存固定 SQL 文本，冲突事实仍在当前事务重新读取。
    static OUTGOING_QUERY: LazyLock<String> = LazyLock::new(|| {
        "SELECT EXISTS(SELECT 1 FROM public.deliveries d
         WHERE d.id IS DISTINCT FROM $3 AND (
           d.payload->'schema_version' IS DISTINCT FROM '1'::jsonb
           OR d.payload->>'world' IS NULL OR d.payload->>'world' NOT IN ('main','isekai')
           OR EXISTS(SELECT 1 FROM public.settlement_obligations origin_obligation
              JOIN public.equipment origin_equipment ON origin_equipment.id=origin_obligation.equipment_id
              WHERE d.origin_kind='settlement' AND d.origin_id=origin_obligation.id::text
                AND origin_equipment.external_equipment_id=$1 AND origin_equipment.world_id=$2)
           OR EXISTS(SELECT 1 FROM public.settlement_recovery_actions origin_action
              JOIN public.settlement_recoveries origin_recovery ON origin_recovery.id=origin_action.recovery_id
              JOIN public.settlement_obligations origin_payment ON origin_payment.id=origin_recovery.payment_obligation_id
              JOIN public.equipment origin_equipment ON origin_equipment.id=origin_payment.equipment_id
              WHERE d.origin_id=origin_action.id::text
                AND origin_equipment.external_equipment_id=$1 AND origin_equipment.world_id=$2)
           OR (NOT (d.id=ANY($4)) AND (
             d.origin_kind IS DISTINCT FROM 'settlement'
             OR NOT COALESCE(d.origin_id ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
                 AND d.origin_id<>'00000000-0000-0000-0000-000000000000',false)
             OR d.payload IS DISTINCT FROM jsonb_build_object(
                 'schema_version',1,'operation','send','world',d.payload->'world',
                 'template',d.payload->'template','variables',d.payload->'variables',
                 'credits',d.payload->'credits','cod',d.payload->'cod',
                 'equipment_secret_refs',d.payload->'equipment_secret_refs')
             OR NOT COALESCE(d.payload->>'template' IN ('settlement_winner_equipment_cod',
                 'settlement_payment_carrier','settlement_equipment_handoff',
                 'settlement_seller_payout','settlement_buyer_refund'),false)
             OR jsonb_typeof(d.payload->'variables') IS DISTINCT FROM 'object'
             OR d.payload->'variables' IS DISTINCT FROM CASE d.payload->>'template'
                 WHEN 'settlement_winner_equipment_cod' THEN CASE
                     WHEN d.payload->'variables' ? 'pricing_mode' THEN CASE
                         WHEN d.payload->'variables'->>'pricing_mode'='hold_curve_v2' THEN jsonb_build_object(
                             'amount',d.payload->'variables'->'amount',
                             'item_name',d.payload->'variables'->'item_name',
                             'reference',d.payload->'variables'->'reference',
                             'hold_ppm',d.payload->'variables'->'hold_ppm',
                             'adjustment_ppm',d.payload->'variables'->'adjustment_ppm',
                             'winning_bid',d.payload->'variables'->'winning_bid',
                             'adjusted_price',d.payload->'variables'->'adjusted_price',
                             'pool_credit',d.payload->'variables'->'pool_credit',
                             'pricing_mode','hold_curve_v2')
                         ELSE NULL END
                     WHEN d.payload->'variables' ? 'hold_ppm' THEN jsonb_build_object(
                         'amount',d.payload->'variables'->'amount',
                         'item_name',d.payload->'variables'->'item_name',
                         'reference',d.payload->'variables'->'reference',
                         'hold_ppm',d.payload->'variables'->'hold_ppm',
                         'adjustment_ppm',d.payload->'variables'->'adjustment_ppm',
                         'winning_bid',d.payload->'variables'->'winning_bid',
                         'adjusted_price',d.payload->'variables'->'adjusted_price',
                         'pool_credit',d.payload->'variables'->'pool_credit')
                     ELSE jsonb_build_object(
                         'amount',d.payload->'variables'->'amount',
                         'item_name',d.payload->'variables'->'item_name',
                         'reference',d.payload->'variables'->'reference') END
                 WHEN 'settlement_equipment_handoff' THEN jsonb_build_object(
                     'item_name',d.payload->'variables'->'item_name',
                     'reference',d.payload->'variables'->'reference')
                 ELSE jsonb_build_object('amount',d.payload->'variables'->'amount',
                     'reference',d.payload->'variables'->'reference') END
             OR d.payload->'variables'->>'reference' IS DISTINCT FROM d.origin_id
             OR EXISTS(SELECT 1 FROM jsonb_each(CASE
                 WHEN jsonb_typeof(d.payload->'variables')='object'
                 THEN d.payload->'variables' ELSE '{}'::jsonb END) variable
                 WHERE jsonb_typeof(variable.value) IS DISTINCT FROM 'string')
             OR NOT COALESCE(jsonb_typeof(d.payload->'credits')='number'
                 AND d.payload->>'credits' ~ '^[0-9]+$'
                 AND d.payload->'credits'<='9223372036854775807'::jsonb,false)
             OR NOT COALESCE(jsonb_typeof(d.payload->'cod')='number'
                 AND d.payload->>'cod' ~ '^[0-9]+$'
                 AND d.payload->'cod'<='9223372036854775807'::jsonb,false)
             OR CASE WHEN jsonb_typeof(d.payload->'equipment_secret_refs')='array'
                 THEN jsonb_array_length(d.payload->'equipment_secret_refs') ELSE -1 END
                <> CASE WHEN d.payload->>'template' IN (
                     'settlement_winner_equipment_cod','settlement_equipment_handoff') THEN 1 ELSE 0 END
             OR EXISTS(SELECT 1 FROM jsonb_array_elements(
                CASE WHEN jsonb_typeof(d.payload->'equipment_secret_refs')='array'
                     THEN d.payload->'equipment_secret_refs' ELSE '[]'::jsonb END) ref
                WHERE ref IS DISTINCT FROM jsonb_build_object(
                       'kind','auction_equipment_key','id',ref->'id')
                   OR jsonb_typeof(ref->'id') IS DISTINCT FROM 'string'
                   OR NOT EXISTS(SELECT 1 FROM public.equipment known
                       WHERE known.id::text=ref->>'id' AND known.world_id=d.payload->>'world')
                   OR EXISTS(SELECT 1 FROM public.equipment target
                       WHERE target.id::text=ref->>'id'
                         AND target.external_equipment_id=$1 AND target.world_id=$2))
             OR d.payload->>'operation' IS DISTINCT FROM 'send')))
           AND NOT EXISTS(SELECT 1 FROM public.settlement_obligation_fulfillments f
               WHERE f.delivery_id=d.id AND f.execution_state='satisfied' AND f.proof_digest IS NOT NULL)
           AND NOT EXISTS(SELECT 1 FROM public.settlement_recovery_actions a
               WHERE a.delivery_id=d.id AND a.execution_state='satisfied' AND a.proof_digest IS NOT NULL)
           AND NOT EXISTS(SELECT 1 FROM public.settlement_legacy_self_reconciliations r
               WHERE /* trusted_legacy_self */))".replace("/* trusted_legacy_self */",
                   crate::database::legacy_self_proof::TRUSTED_DELIVERY_PREDICATE)
    });
    // 替换值仅来自已审查的编译期 SQL 常量，不插入业务文本。
    let outgoing: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(OUTGOING_QUERY.as_str()))
        .bind(c.external_equipment_id)
        .bind(c.equipment_world.as_str())
        .bind(recovered_handoff)
        .bind(&unrelated_attachments)
        .fetch_one(&mut **tx)
        .await
        .map_err(sql_error)?;
    if outgoing {
        return Err(FundingError::Unproven);
    }
    Ok(())
}

/// 普通outbox的闭集schema；不能把缺失装备引用字段本身当作无装备证明。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AttachmentPayload {
    schema_version: u8,
    operation: String,
    world: String,
    external_mail_id: String,
    template: Option<String>,
    result_code: Option<String>,
    body: Option<String>,
    user_key_user_id: Option<Uuid>,
    credits: i64,
    cod: i64,
    attachment_ids: Vec<String>,
}

/// 资金责任预览仅忽略闭集普通入站通知的零资产历史，不将其标记成功或恢复写入权限。
pub(crate) fn assetless_mail_notification(
    value: &serde_json::Value,
    origin_id: &str,
    command_id: Uuid,
) -> bool {
    let Ok(payload) = AttachmentPayload::deserialize(value) else {
        return false;
    };
    payload.schema_version == 1
        && matches!(payload.world.as_str(), "main" | "isekai")
        && payload.operation == "send"
        && payload.credits == 0
        && payload.cod == 0
        && payload.attachment_ids.is_empty()
        && auction_application::valid_external_positive_id(&payload.external_mail_id)
        && valid_attachment_origin(&payload, "mail_intake", origin_id, command_id, false)
        && valid_notification(&payload, "mail_intake")
}

fn unrelated_attachment_payload(
    value: &serde_json::Value,
    origin: &str,
    origin_id: &str,
    command_id: Uuid,
    forum_notice_owner: bool,
    claim: &AssetConflictTarget<'_>,
) -> bool {
    let Ok(payload) = AttachmentPayload::deserialize(value) else {
        return false;
    };
    if payload.schema_version != 1
        || !matches!(payload.world.as_str(), "main" | "isekai")
        || !valid_line(&payload.external_mail_id, 256)
        || payload.credits < 0
        || payload.cod < 0
        || payload.attachment_ids.len() > 1000
        || payload.attachment_ids.iter().any(|eid| {
            eid.is_empty()
                || eid.len() > 32
                || !eid.bytes().all(|byte| byte.is_ascii_digit())
                || eid.bytes().all(|byte| byte == b'0')
        })
        || !unique_attachment_ids(&payload.attachment_ids)
        || !valid_attachment_origin(&payload, origin, origin_id, command_id, forum_notice_owner)
    {
        return false;
    }
    let valid_operation = match payload.operation.as_str() {
        "send" => {
            payload.credits == 0
                && payload.cod == 0
                && payload.attachment_ids.is_empty()
                && valid_notification(&payload, origin)
        }
        "return" | "take" => {
            payload.template.is_none()
                && payload.result_code.is_none()
                && payload.user_key_user_id.is_none()
                && payload.body.is_none()
                && match (origin, payload.operation.as_str()) {
                    ("mail_intake", _) => payload.credits == 0 && payload.cod == 0,
                    ("settlement_recovery_action", "take") => {
                        payload.attachment_ids.windows(2).all(|pair| pair[0] < pair[1])
                    }
                    _ => false,
                }
        }
        _ => false,
    };
    // attachment_ids是外部EID，不是equipment UUID；按十进制值比较，兼容既有数字字符串。
    valid_operation
        && (payload.world != claim.equipment_world.as_str()
            || !payload.attachment_ids.iter().any(|eid| {
                eid.trim_start_matches('0') == claim.external_equipment_id.trim_start_matches('0')
            }))
}

fn valid_attachment_origin(
    payload: &AttachmentPayload,
    origin: &str,
    origin_id: &str,
    command_id: Uuid,
    forum_notice_owner: bool,
) -> bool {
    if payload.template.as_deref() == Some("forum_auction_notice") {
        // 完整origin已按真实notify账本重建；不能只凭forum:前缀或模板进入无装备集合。
        return forum_notice_owner
            && origin == "mail_intake"
            && !command_id.is_nil()
            && origin_id == payload.external_mail_id;
    }
    match origin {
        "mail_intake" => {
            let legacy_operation = match payload.operation.as_str() {
                "send" => "delivery-notice",
                "return" => "delivery-return",
                "take" => "delivery-take",
                _ => return false,
            };
            origin_id
                == format!("{}:{}:{}", payload.world, payload.external_mail_id, payload.operation)
                || origin_id
                    == format!("{}:{}:{legacy_operation}", payload.world, payload.external_mail_id)
        }
        "settlement_recovery_action" => {
            origin_id.strip_prefix("recovery:").is_some_and(canonical_uuid)
                && (payload.operation != "send" || payload.world == "main")
        }
        "operations_alert" => {
            payload.external_mail_id.strip_prefix("operations-alert:").is_some_and(|batch_id| {
                canonical_uuid(batch_id) && origin_id == format!("operations_alert:{batch_id}")
            })
        }
        "lot_watch" => {
            !command_id.is_nil()
                && origin_id == format!("lot_watch:{command_id}")
                && payload.external_mail_id == command_id.to_string()
        }
        _ => false,
    }
}

fn valid_notification(payload: &AttachmentPayload, origin: &str) -> bool {
    let code = payload.result_code.as_deref();
    if !code.is_some_and(|value| valid_line(value, 256)) {
        return false;
    }
    let no_key = payload.user_key_user_id.is_none();
    if matches!(origin, "operations_alert" | "lot_watch") {
        let expected = if origin == "operations_alert" {
            ("operations_alert", "operations_alert")
        } else {
            ("lot_watch", "outbid")
        };
        return payload.template.as_deref() == Some(expected.0)
            && code == Some(expected.1)
            && no_key
            && auction_application::valid_notification_body(payload.body.as_deref());
    }
    if payload.body.is_some() {
        return false;
    }
    if origin == "settlement_recovery_action" {
        return payload.template.as_deref() == Some("recovery_payment_reminder")
            && code == Some("payment_overdue")
            && no_key;
    }
    if origin != "mail_intake" {
        return false;
    }
    match payload.template.as_deref() {
        Some("forum_auction_notice") => code == Some("forum_auction_notice") && no_key,
        Some("register_key") => {
            payload.user_key_user_id.is_some_and(|id| !id.is_nil())
                && matches!(code, Some("registered" | "recovered"))
        }
        Some("register_result") => {
            no_key && matches!(code, Some("blacklisted" | "identity_conflict"))
        }
        Some("rebind_result") => {
            no_key
                && matches!(
                    code,
                    Some(
                        "rebound"
                            | "unchanged"
                            | "blacklisted"
                            | "identity_conflict"
                            | "invalid_key"
                            | "missing_key"
                    )
                )
        }
        Some(
            "auction_assets_rejected"
            | "auction_no_attachments"
            | "auction_equipment_rejected"
            | "user_assets_rejected",
        ) => no_key,
        _ => false,
    }
}

// 调用方先验证数量及单项格式；去重仍按原字符串，不合并带前导零的不同历史身份。
fn unique_attachment_ids(ids: &[String]) -> bool {
    let mut seen = HashSet::with_capacity(ids.len());
    ids.iter().all(|id| seen.insert(id.as_str()))
}

fn canonical_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}
