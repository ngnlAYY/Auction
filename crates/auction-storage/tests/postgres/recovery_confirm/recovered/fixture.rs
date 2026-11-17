//! 待 Admin 确认 `equipment_recovered` 的 isekai 成交夹具，以及退款资金转换所需的资金账户、邮件
//! 运行时与 opaque 证据。
//!
//! 在 isekai 回收夹具之上改写为：载体付款已满足（Sent 领取证明）、交付已在 isekai 寄出并有可信
//! Sent 证明、交付被系统退回且退件已被本恢复记录消费、本轮 Take 已按退件 MID 在 isekai 取回装备、
//! 恢复记录停在 `reclaiming` 并带同一 custody 证明。另建 isekai 草稿接收场次，使确认可以直接重列
//! 并进入 `refunding`。只用直接 SQL 写事实，被测路径全部走生产端口。

use auction_application::settlement::{
    INTERNAL_EVIDENCE_PROTOCOL, InternalEvidencePurpose, MailAccountEvidenceBinding,
    MailRuntimeBinding, SettlementFulfillmentClaim, SettlementInternalEvidenceSnapshot,
    VerifiedMailEquipmentPresence,
};
use auction_domain::SettlementWorld;
use chrono::{Duration, Utc};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::fixture::{Sale, SaleSpec, insert_reclaiming_sale};

/// 资金账户与证据身份共用的认证 UID。
pub(crate) const FUNDING_UID: &str = "7003";

/// 改写后的成交与本轮收回身份。
pub(crate) struct Recovered {
    pub(crate) sale: Sale,
    pub(crate) handoff: Uuid,
    pub(crate) handoff_delivery: Uuid,
    pub(crate) take_delivery: Uuid,
    pub(crate) lot_id: Uuid,
}

/// 写入 isekai 已付款、交付被退回且装备已收回的成交；`sequence` 区分外部 ID。
pub(crate) async fn insert_recovered_sale(pool: &PgPool, sequence: i64) -> Recovered {
    let sale = insert_reclaiming_sale(
        pool,
        SaleSpec {
            world: "isekai",
            sequence,
            cod_return_count: 1,
            cod_return_limit: 3,
            blacklist_satisfied: false,
        },
    )
    .await;
    let handoff = sale.handoff.expect("isekai 夹具带交付义务");
    let (lot_id, equipment_id, external_equipment_id, created_at): (
        Uuid,
        Uuid,
        String,
        chrono::DateTime<Utc>,
    ) = sqlx::query_as(
        "SELECT recovery.lot_id, recovery.equipment_id, equipment.external_equipment_id,
                recovery.created_at
         FROM public.settlement_recoveries AS recovery
         JOIN public.equipment AS equipment ON equipment.id = recovery.equipment_id
         WHERE recovery.id = $1",
    )
    .bind(sale.recovery_id)
    .fetch_one(pool)
    .await
    .expect("读取恢复记录身份");

    // 买家已付载体 COD：付款履约以 Sent 领取证明满足，交付义务随之就绪。
    sqlx::query(
        "UPDATE public.settlement_obligation_fulfillments
         SET execution_state = 'satisfied', proof_kind = 'mooglemail_sent_collected',
             proof_reference = '1234567', proof_digest = decode(repeat('c1', 32), 'hex'),
             proof_observed_at = $2, completed_at = $2, updated_at = $2
         WHERE obligation_id = $1",
    )
    .bind(sale.payment)
    .bind(created_at + Duration::minutes(10))
    .execute(pool)
    .await
    .expect("载体付款已满足");
    sqlx::query("UPDATE public.settlement_obligations SET business_state = 'ready' WHERE id = $1")
        .bind(handoff)
        .execute(pool)
        .await
        .expect("交付义务就绪");

    // 交付已在 isekai 寄出并有可信 Sent 证明，履约等待领取。
    let sent_at = created_at + Duration::minutes(15);
    let handoff_delivery = Uuid::new_v4();
    let prepare = Uuid::new_v4();
    let activate = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status,
             fencing_version, attempt_count, claimed_at, sending_at, completed_at,
             external_reference, created_at, updated_at, result_metadata,
             activation_command_id, activation_request_digest, activated_at)
         VALUES ($1, $2, decode(repeat('d1', 32), 'hex'), 'settlement', $3, 'Recovery buyer',
                 jsonb_build_object('schema_version', 1, 'operation', 'send', 'world', 'isekai',
                     'template', 'settlement_equipment_handoff',
                     'variables', jsonb_build_object('item_name', 'Recovery confirm equipment',
                         'reference', $3),
                     'credits', 0, 'cod', 0,
                     'equipment_secret_refs', jsonb_build_array(jsonb_build_object(
                         'kind', 'auction_equipment_key', 'id', $4::text))),
                 'succeeded', 1, 1, $5, $5, $5, $6, $5, $5,
                 '{\"operation\":\"send\",\"proof\":\"sent_exact_payload_v1\",
                   \"external_reference_proven\":true}'::jsonb,
                 $7, decode(repeat('d2', 32), 'hex'), $5)",
    )
    .bind(handoff_delivery)
    .bind(prepare)
    .bind(handoff.to_string())
    .bind(equipment_id)
    .bind(sent_at)
    .bind(format!("mooglemail:isekai:send:{}", 880_000_000 + sequence % 1_000_000))
    .bind(activate)
    .execute(pool)
    .await
    .expect("写入原交付投递");
    sqlx::query(
        "UPDATE public.settlement_obligation_fulfillments
         SET execution_state = 'enqueued', delivery_id = $2, prepare_command_id = $3,
             prepare_request_digest = decode(repeat('d1', 32), 'hex'),
             activate_command_id = $4, activate_request_digest = decode(repeat('d2', 32), 'hex'),
             updated_at = $5
         WHERE obligation_id = $1",
    )
    .bind(handoff)
    .bind(handoff_delivery)
    .bind(prepare)
    .bind(activate)
    .bind(sent_at)
    .execute(pool)
    .await
    .expect("交付履约等待领取");

    // 系统把交付退回 isekai 收件箱，恢复流程已消费这封退件。
    let return_mail = Uuid::new_v4();
    let return_mid = format!("{}", 660_000 + sequence % 100_000);
    let returned_at = created_at + Duration::minutes(40);
    sqlx::query(
        "INSERT INTO public.inbound_mail
            (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
             received_at, status, decision_code, decision_metadata, processed_at, sender, subject)
         VALUES ($1, 'recovery-confirm-isekai', $2, 'isekai', 'delivery',
                 decode(repeat('d3', 32), 'hex'), $3, 'accepted', 'settlement_recovery_returned',
                 $4, $3, 'MoogleMail', $5)",
    )
    .bind(return_mail)
    .bind(&return_mid)
    .bind(returned_at)
    .bind(json!({
        "obligation_id": handoff.to_string(),
        "credits": 0,
        "cod": 0,
        "attachment_ids": [external_equipment_id.clone()],
    }))
    .bind(format!("[RTS] Auction equipment delivery [{handoff}]"))
    .execute(pool)
    .await
    .expect("写入交付退件");
    sqlx::query(
        "INSERT INTO public.settlement_recovery_return_consumptions
            (return_mail_id, recovery_id, recorded_at) VALUES ($1, $2, $3)",
    )
    .bind(return_mail)
    .bind(sale.recovery_id)
    .bind(returned_at)
    .execute(pool)
    .await
    .expect("恢复流程消费交付退件");

    // 本轮 Take 改为按交付退件 MID 在 isekai 取回装备，恢复记录指向同一退件与证明。
    let take_delivery: Uuid = sqlx::query_scalar(
        "UPDATE public.settlement_recovery_actions
         SET kind = 'recover_equipment_take', source_mail_id = $2,
             expected_attachment_ids = ARRAY[$3], proof_reference = $4
         WHERE recovery_id = $1 AND kind = 'cancel_carrier_take'
         RETURNING delivery_id",
    )
    .bind(sale.recovery_id)
    .bind(return_mail)
    .bind(&external_equipment_id)
    .bind(&return_mid)
    .fetch_one(pool)
    .await
    .expect("改写为装备收回动作");
    sqlx::query(
        "UPDATE public.deliveries
         SET payload = jsonb_build_object('schema_version', 1, 'operation', 'take',
                 'world', 'isekai', 'external_mail_id', $2::text),
             external_reference = 'mooglemail:isekai:take:' || $2
         WHERE id = $1",
    )
    .bind(take_delivery)
    .bind(&return_mid)
    .execute(pool)
    .await
    .expect("Take 投递在 isekai 取回");
    sqlx::query(
        "UPDATE public.settlement_recoveries
         SET return_mail_id = $2, return_external_mail_id = $3, custody_proof_reference = $3
         WHERE id = $1",
    )
    .bind(sale.recovery_id)
    .bind(return_mail)
    .bind(&return_mid)
    .execute(pool)
    .await
    .expect("恢复记录指向交付退件");

    Recovered { sale, handoff, handoff_delivery, take_delivery, lot_id }
}

/// isekai 草稿接收场次（届次与成交场次错开）：确认后装备直接重列，恢复记录进入 `refunding`。
pub(crate) async fn insert_relist_target(pool: &PgPool, sequence: i64) {
    let auction = Uuid::new_v4();
    let at = Utc::now() - Duration::hours(1);
    let mut transaction = pool.begin().await.expect("开启草稿目标事务");
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, preview_opened_at, opened_at, policy_snapshot)
         SELECT $1, 'isekai', $2, 'Recovery relist target', 'draft', $3, $3, $3, $3, $3, $3,
                policy_snapshot
         FROM public.auctions WHERE world_id = 'isekai' AND status = 'settled' LIMIT 1",
    )
    .bind(auction)
    .bind(sequence + 1_000_000)
    .bind(at)
    .execute(&mut *transaction)
    .await
    .expect("写入草稿场次");
    sqlx::query(
        "INSERT INTO public.auction_intake_targets (world_id, auction_id, target_status)
         VALUES ('isekai', $1, 'draft')",
    )
    .bind(auction)
    .execute(&mut *transaction)
    .await
    .expect("写入草稿接收目标");
    transaction.commit().await.expect("提交草稿目标");
}

/// 记账模式的资金账户与该成交的资金化事实（卖方用途 100、税 0），以及证据事务所需的邮件运行时
/// generation 与 active 共享邮件凭据；返回凭据 ID。
pub(crate) async fn insert_funding(pool: &PgPool, recovered: &Recovered) -> Uuid {
    sqlx::query(
        "INSERT INTO public.settlement_funding_accounts (uid, status, state_version, accounting_mode)
         VALUES ($1, 'ready', 1, 'record_only')",
    )
    .bind(FUNDING_UID)
    .execute(pool)
    .await
    .expect("写入资金账户");
    sqlx::query(
        "INSERT INTO public.settlement_funding_lots (lot_id, uid, payment_obligation_id, source_kind)
         VALUES ($1, $2, $3, 'collected')",
    )
    .bind(recovered.lot_id)
    .bind(FUNDING_UID)
    .bind(recovered.sale.payment)
    .execute(pool)
    .await
    .expect("写入资金化 lot");
    for (purpose, obligation, amount) in
        [("seller", Some(recovered.sale.seller), 100_i64), ("tax", None, 0)]
    {
        sqlx::query(
            "INSERT INTO public.settlement_funding_allocations
                (id, uid, purpose, lot_id, obligation_id, amount)
             VALUES (gen_random_uuid(), $1, $2, $3, $4, $5)",
        )
        .bind(FUNDING_UID)
        .bind(purpose)
        .bind(recovered.lot_id)
        .bind(obligation)
        .bind(amount)
        .execute(pool)
        .await
        .expect("写入资金分配");
    }
    sqlx::query(
        "INSERT INTO public.mail_runtime_generation (scope, generation, activated_at)
         VALUES ('mail_cookie', 1, clock_timestamp())",
    )
    .execute(pool)
    .await
    .expect("写入邮件运行时 generation");
    let credential = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.credential_keys
            (id, credential_kind, scope, label, ciphertext, nonce, encryption_key_id,
             fingerprint, status, state_version)
         VALUES ($1, 'mail_cookie', 'mooglemail:shared', 'recovered', $2, $3, 'test-key',
                 'fingerprint-recovered', 'active', 1)",
    )
    .bind(credential)
    .bind(vec![1_u8])
    .bind(vec![0_u8; 12])
    .execute(pool)
    .await
    .expect("写入共享邮件凭据");
    credential
}

/// 绑定当前 claim 与指定操作身份的 opaque 证据：同一认证账号在 Main 的身份，加上同账号在 isekai
/// 对该装备的 organize 正向观察；记账模式不观察余额。
pub(crate) fn evidence(
    operation_id: Uuid,
    claim: &SettlementFulfillmentClaim,
    credential_id: Uuid,
) -> SettlementInternalEvidenceSnapshot {
    let observed = Utc::now().max(claim.claimed_at);
    let mut identity = MailAccountEvidenceBinding {
        credential_id,
        credential_version: 1,
        runtime: MailRuntimeBinding { generation: 1 },
        authenticated_uid: FUNDING_UID.to_owned(),
        player_name: "Recovery Bot".to_owned(),
        world: SettlementWorld::Main,
        observed_at: observed,
        protocol_version: INTERNAL_EVIDENCE_PROTOCOL,
        digest: [0; 32],
    };
    identity.digest = identity.compute_digest();
    let mut binding = identity.clone();
    binding.world = SettlementWorld::Isekai;
    binding.digest = binding.compute_digest();
    let mut equipment = VerifiedMailEquipmentPresence {
        binding,
        equipment_id: claim.equipment_id,
        external_equipment_id: claim.external_equipment_id.clone(),
        category: "weapon_1handed".to_owned(),
        observed_at: observed,
        digest: [0; 32],
    };
    equipment.digest = equipment.compute_digest();
    let mut snapshot = SettlementInternalEvidenceSnapshot {
        operation_id,
        purpose: InternalEvidencePurpose::SettlementClaim {
            obligation_id: claim.obligation_id.as_uuid(),
            claim_token: claim.claim_token,
            claim_fencing: claim.claim_fencing,
            state_version: claim.state_version,
        },
        identity,
        equipment: Some(equipment),
        main_cash: None,
        sent_collection: None,
        observed_from: observed,
        expires_at: observed + Duration::seconds(60),
        digest: [0; 32],
    };
    snapshot.digest = snapshot.compute_digest();
    snapshot
}
