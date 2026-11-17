//! 待 Admin 确认的 COD 退回成交夹具：已结算场次、带寄售来源的 lot、冻结结算事实、全部义务与
//! 履约、系统退件、已满足的收回动作与（可选的）拉黑动作，以及停在 `reclaiming` 的恢复记录。
//! 只用直接 SQL 写事实，被测路径全部走生产端口。

use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

/// 夹具里的成交与恢复身份。
pub(crate) struct Sale {
    pub(crate) auction_id: Uuid,
    pub(crate) lot_id: Uuid,
    pub(crate) payment: Uuid,
    pub(crate) handoff: Option<Uuid>,
    pub(crate) seller: Uuid,
    pub(crate) refund: Uuid,
    pub(crate) recovery_id: Uuid,
    pub(crate) payment_delivery: Uuid,
    pub(crate) state_version: i64,
}

/// 夹具参数：世界、序号（区分外部 ID）、已计入的退回次数与冻结上限、拉黑动作是否已满足。
pub(crate) struct SaleSpec {
    pub(crate) world: &'static str,
    pub(crate) sequence: i64,
    pub(crate) cod_return_count: i32,
    pub(crate) cod_return_limit: i32,
    pub(crate) blacklist_satisfied: bool,
}

fn digest(seed: u8) -> [u8; 32] {
    [seed; 32]
}

pub(crate) async fn insert_reclaiming_sale(pool: &PgPool, spec: SaleSpec) -> Sale {
    let at = Utc::now() - Duration::hours(2);
    let world = spec.world;
    let isekai = world == "isekai";
    let winner = Uuid::new_v4();
    let auction_id = Uuid::new_v4();
    let lot_id = Uuid::new_v4();
    let equipment_id = Uuid::new_v4();
    let external_equipment_id = format!("{}", 800_000 + spec.sequence % 100_000);
    let origin_command = Uuid::new_v4();
    let submission_command = Uuid::new_v4();
    let settlement_command = Uuid::new_v4();
    let submission_id = Uuid::new_v4();

    let mut key_hash = [0_u8; 32];
    key_hash[..16].copy_from_slice(winner.as_bytes());
    key_hash[16..].copy_from_slice(winner.as_bytes());
    sqlx::query("INSERT INTO public.users (id, username, public_key_hash) VALUES ($1, $2, $3)")
        .bind(winner)
        .bind(format!("recovery-confirm-{winner}"))
        .bind(key_hash.as_slice())
        .execute(pool)
        .await
        .expect("写入买家");
    sqlx::query(
        "INSERT INTO public.equipment
            (id, world_id, external_equipment_id, name, category, attributes, collected_at)
         VALUES ($1, $2, $3, 'Recovery confirm equipment', 'weapon', '{}'::jsonb, $4)",
    )
    .bind(equipment_id)
    .bind(world)
    .bind(&external_equipment_id)
    .bind(at)
    .execute(pool)
    .await
    .expect("写入装备");
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, preview_opened_at, opened_at, policy_snapshot)
         VALUES ($1, $2, $3, 'Recovery confirm auction', 'settled', $4, $4, $4, $4, $4, $4, $5)",
    )
    .bind(auction_id)
    .bind(world)
    .bind(spec.sequence)
    .bind(at)
    .bind(json!({
        "schema_version": 5,
        "bidding_version": 2,
        "start_price": 100,
        "bid_increment_ppm": 100_000,
        "minimum_increment": 100,
        "soft_close_window_seconds": 900,
        "soft_close_extension_seconds": 900,
        "proxy_enabled": true,
        "proxy_default_mode": "fast",
        "proxy_slow_interval_seconds": 600,
        "proxy_max_outbid_uses": 3,
        "final_phase_lead_seconds": 900,
        "scheduled_end_at": at,
        "settlement": {"version": 1, "mode": "hold_curve_v1", "anchors": []},
        "recovery": {"version": 1, "payment_timeout_seconds": 604_800, "mode": "remind_v1"},
    }))
    .execute(pool)
    .await
    .expect("写入场次");
    for (command_id, kind, request) in [
        (origin_command, "recovery-confirm-settle", 31_u8),
        (submission_command, "admin_intake_item", 32),
        (settlement_command, "recovery-confirm-settlement", 33),
    ] {
        sqlx::query(
            "INSERT INTO public.auction_commands
                (command_id, command_kind, aggregate_id, request_hash, result_code)
             VALUES ($1, $2, $3, $4, 'accepted')",
        )
        .bind(command_id)
        .bind(kind)
        .bind(auction_id)
        .bind(digest(request).as_slice())
        .execute(pool)
        .await
        .expect("写入命令账本");
    }
    let mut fingerprint = [0_u8; 32];
    fingerprint[..16].copy_from_slice(submission_id.as_bytes());
    fingerprint[16..].copy_from_slice(submission_command.as_bytes());
    sqlx::query(
        "INSERT INTO public.auction_submissions
            (id, source_command_id, auction_id, seller, received_at,
             source_kind, source_reference, source_fingerprint)
         VALUES ($1, $2, $3, 'Recovery seller', $4, 'admin', $5, $6)",
    )
    .bind(submission_id)
    .bind(submission_command)
    .bind(auction_id)
    .bind(at - Duration::hours(1))
    .bind(format!("recovery-confirm-{submission_id}"))
    .bind(fingerprint.as_slice())
    .execute(pool)
    .await
    .expect("写入寄售来源");
    sqlx::query(
        "INSERT INTO public.lots
            (id, auction_id, equipment_id, lot_number, status, start_price, current_price,
             effective_end_at, category_key, category_number, submission_id)
         VALUES ($1, $2, $3, 1, 'sold', 100, 100, $4, 'weapon', 1, $5)",
    )
    .bind(lot_id)
    .bind(auction_id)
    .bind(equipment_id)
    .bind(at)
    .bind(submission_id)
    .execute(pool)
    .await
    .expect("写入 lot");
    sqlx::query(
        "INSERT INTO public.auction_settlements
            (auction_id, command_id, policy_version, policy_mode, pool_enabled,
             opening_pool_balance, closing_pool_balance, settled_at)
         VALUES ($1, $2, 0, 'legacy_zero_v0', false, 0, 0, $3)",
    )
    .bind(auction_id)
    .bind(settlement_command)
    .bind(at)
    .execute(pool)
    .await
    .expect("写入场次结算");
    sqlx::query(
        "INSERT INTO public.lot_settlements
            (lot_id, end_price, listed_price, buyer_amount, seller_amount, tax_amount,
             compensation_amount, hold_ppm, adjustment_ppm)
         VALUES ($1, 100, 100, 100, 100, 0, 0, 0, 0)",
    )
    .bind(lot_id)
    .execute(pool)
    .await
    .expect("写入 lot 结算");

    let payment = Uuid::new_v4();
    let handoff = isekai.then(Uuid::new_v4);
    let seller = Uuid::new_v4();
    let refund = Uuid::new_v4();
    let (payment_kind, carrier_credit) =
        if isekai { ("winner_payment_carrier_cod", 1) } else { ("winner_equipment_cod", 0) };
    let mut obligations = vec![(payment, payment_kind, "ready", true, 100, carrier_credit, None)];
    if let Some(handoff) = handoff {
        obligations.push((
            handoff,
            "winner_equipment_handoff",
            "blocked",
            true,
            0,
            0,
            Some(payment),
        ));
    }
    obligations.push((
        seller,
        "seller_payout",
        "blocked",
        false,
        100,
        0,
        Some(handoff.unwrap_or(payment)),
    ));
    obligations.push((refund, "buyer_refund", "contingent", true, 100, 0, Some(payment)));
    for (id, kind, state, to_user, amount, carrier, prerequisite) in obligations {
        sqlx::query(
            "INSERT INTO public.settlement_obligations
                (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
                 obligation_kind, business_state, recipient_user_id, recipient_game_identity,
                 amount, carrier_credit, equipment_id, prerequisite_obligation_id)
             VALUES ($1, $2, $3, $4, $5, 1, $6, $7, $8, $9, $10, $11, $12, $13)",
        )
        .bind(id)
        .bind(auction_id)
        .bind(lot_id)
        .bind(world)
        .bind(origin_command)
        .bind(kind)
        .bind(state)
        .bind(to_user.then_some(winner))
        .bind((!to_user).then_some("Recovery seller"))
        .bind(amount)
        .bind(carrier)
        .bind(equipment_id)
        .bind(prerequisite)
        .execute(pool)
        .await
        .expect("写入结算义务");
    }

    let payment_delivery = insert_payment_delivery(pool, payment, isekai, at).await;
    let mut pending = vec![(seller, "main"), (refund, "main")];
    if let Some(handoff) = handoff {
        pending.push((handoff, "isekai"));
    }
    for (obligation, dispatch_world) in pending {
        sqlx::query(
            "INSERT INTO public.settlement_obligation_fulfillments
                (obligation_id, execution_state, dispatch_world, delivery_operation,
                 created_at, updated_at)
             VALUES ($1, 'pending', $2, 'send', $3, $3)",
        )
        .bind(obligation)
        .bind(dispatch_world)
        .bind(at)
        .execute(pool)
        .await
        .expect("写入尚未开始的履约");
    }

    let recovery_id = Uuid::new_v4();
    let return_external_mail_id = format!("{}", 550_000 + spec.sequence % 100_000);
    let return_mail_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.inbound_mail
            (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
             received_at, status, decision_code, decision_metadata, processed_at, sender, subject)
         VALUES ($1, 'recovery-confirm', $2, 'main', 'delivery', $3, $4, 'accepted',
                 'settlement_recovery_returned', $5, $4, 'MoogleMail', $6)",
    )
    .bind(return_mail_id)
    .bind(&return_external_mail_id)
    .bind(digest(41).as_slice())
    .bind(at + Duration::minutes(30))
    .bind(json!({
        "obligation_id": payment.to_string(),
        "credits": carrier_credit,
        "cod": 0,
        "attachment_ids": if isekai { Vec::new() } else { vec![external_equipment_id.clone()] },
    }))
    .bind(if isekai {
        format!("[RTS] Auction payment request [{payment}]")
    } else {
        format!("[RTS] Auction equipment delivery [{payment}]")
    })
    .execute(pool)
    .await
    .expect("写入系统退件");
    let custody_observed_at = at + Duration::minutes(45);
    let state_version = 5_i64;
    sqlx::query(
        "INSERT INTO public.settlement_recoveries
            (id, auction_id, lot_id, world_id, equipment_id, winner_user_id,
             payment_obligation_id, equipment_obligation_id, seller_payout_obligation_id,
             buyer_refund_obligation_id, policy_version, payment_deadline_at, stage,
             state_version, return_mail_id, return_external_mail_id, custody_proof_kind,
             custody_proof_reference, custody_proof_digest, custody_proof_observed_at,
             cod_return_count, cod_return_limit, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 1, $11 + interval '7 days', 'reclaiming',
                 $12, $13, $14, 'delivery_recovery_take_succeeded', $14, $15, $16, $17, $18,
                 $11, $16)",
    )
    .bind(recovery_id)
    .bind(auction_id)
    .bind(lot_id)
    .bind(world)
    .bind(equipment_id)
    .bind(winner)
    .bind(payment)
    .bind(handoff)
    .bind(seller)
    .bind(refund)
    .bind(at)
    .bind(state_version)
    .bind(return_mail_id)
    .bind(&return_external_mail_id)
    .bind(digest(42).as_slice())
    .bind(custody_observed_at)
    .bind(spec.cod_return_count)
    .bind(spec.cod_return_limit)
    .execute(pool)
    .await
    .expect("写入回收中的恢复记录");
    sqlx::query(
        "INSERT INTO public.settlement_recovery_return_consumptions
            (return_mail_id, recovery_id, recorded_at) VALUES ($1, $2, $3)",
    )
    .bind(return_mail_id)
    .bind(recovery_id)
    .bind(at + Duration::minutes(31))
    .execute(pool)
    .await
    .expect("写入退件消费事实");

    let take_delivery = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, claimed_at, sending_at, completed_at,
             external_reference, created_at, updated_at, result_metadata)
         VALUES ($1, $2, $3, 'settlement_recovery_action', $4, 'MoogleMail',
                 jsonb_build_object('schema_version', 1, 'operation', 'take', 'world', 'main',
                     'external_mail_id', $5::text),
                 'succeeded', 1, 1, $6, $6, $6, $7, $6, $6, '{\"operation\":\"take\"}'::jsonb)",
    )
    .bind(take_delivery)
    .bind(Uuid::new_v4())
    .bind(digest(43).as_slice())
    .bind(format!("recovery:{recovery_id}"))
    .bind(&return_external_mail_id)
    .bind(custody_observed_at)
    .bind(format!("mooglemail:main:take:{return_external_mail_id}"))
    .execute(pool)
    .await
    .expect("写入收回投递");
    let take_kind = if isekai { "cancel_carrier_take" } else { "reclaim_equipment_take" };
    sqlx::query(
        "INSERT INTO public.settlement_recovery_actions
            (id, recovery_id, kind, ordinal, execution_state, state_version, source_mail_id,
             delivery_id, proof_kind, proof_reference, proof_digest, proof_observed_at,
             created_at, updated_at, completed_at)
         VALUES ($1, $2, $3, 1, 'satisfied', 3, $4, $5, 'delivery_recovery_take_succeeded',
                 $6, $7, $8, $9, $8, $8)",
    )
    .bind(Uuid::new_v4())
    .bind(recovery_id)
    .bind(take_kind)
    .bind(return_mail_id)
    .bind(take_delivery)
    .bind(&return_external_mail_id)
    .bind(digest(42).as_slice())
    .bind(custody_observed_at)
    .bind(at + Duration::minutes(31))
    .execute(pool)
    .await
    .expect("写入已满足的收回动作");
    if spec.blacklist_satisfied {
        sqlx::query(
            "INSERT INTO public.settlement_recovery_actions
                (id, recovery_id, kind, ordinal, execution_state, state_version, proof_kind,
                 proof_reference, proof_digest, proof_observed_at, reason, created_at,
                 updated_at, completed_at)
             VALUES ($1, $2, 'buyer_blacklist', 2, 'satisfied', 2, 'users_blacklist_applied',
                     $3, $4, $5, 'COD returned repeatedly', $6, $5, $5)",
        )
        .bind(Uuid::new_v4())
        .bind(recovery_id)
        .bind(winner.to_string())
        .bind(digest(44).as_slice())
        .bind(custody_observed_at)
        .bind(at + Duration::minutes(31))
        .execute(pool)
        .await
        .expect("写入已满足的拉黑动作");
    }

    Sale {
        auction_id,
        lot_id,
        payment,
        handoff,
        seller,
        refund,
        recovery_id,
        payment_delivery,
        state_version,
    }
}

/// 已寄出（Sent 证明成功）、随后被退回的原付款投递与 enqueued 履约，带完整 prepare/activate 身份。
async fn insert_payment_delivery(
    pool: &PgPool,
    payment: Uuid,
    isekai: bool,
    at: DateTime<Utc>,
) -> Uuid {
    let delivery_id = Uuid::new_v4();
    let prepare_command_id = Uuid::new_v4();
    let activate_command_id = Uuid::new_v4();
    let template =
        if isekai { "settlement_payment_carrier" } else { "settlement_winner_equipment_cod" };
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, claimed_at, sending_at, completed_at,
             external_reference, created_at, updated_at, result_metadata,
             activation_command_id, activation_request_digest, activated_at)
         VALUES ($1, $2, decode(repeat('a1', 32), 'hex'), 'settlement', $3, 'Recovery buyer',
                 jsonb_build_object('schema_version', 1, 'operation', 'send', 'world', 'main',
                     'template', $4::text, 'credits', 0, 'cod', 100,
                     'variables', jsonb_build_object('reference', $3),
                     'equipment_secret_refs', '[]'::jsonb),
                 'succeeded', 2, 1, $5, $5, $5, $6, $5, $5, '{\"operation\":\"send\"}'::jsonb,
                 $7, decode(repeat('a2', 32), 'hex'), $5)",
    )
    .bind(delivery_id)
    .bind(prepare_command_id)
    .bind(payment.to_string())
    .bind(template)
    .bind(at + Duration::minutes(5))
    .bind(format!("mooglemail:main:send:{}", 770_000_000 + delivery_id.as_u128() % 1_000_000))
    .bind(activate_command_id)
    .execute(pool)
    .await
    .expect("写入原付款投递");
    sqlx::query(
        "INSERT INTO public.settlement_obligation_fulfillments
            (obligation_id, execution_state, state_version, dispatch_world, delivery_operation,
             delivery_id, prepare_command_id, prepare_request_digest, activate_command_id,
             activate_request_digest, created_at, updated_at)
         VALUES ($1, 'enqueued', 3, 'main', 'send', $2, $3, decode(repeat('a1', 32), 'hex'),
                 $4, decode(repeat('a2', 32), 'hex'), $5, $5)",
    )
    .bind(payment)
    .bind(delivery_id)
    .bind(prepare_command_id)
    .bind(activate_command_id)
    .bind(at + Duration::minutes(5))
    .execute(pool)
    .await
    .expect("写入原付款履约");
    delivery_id
}
