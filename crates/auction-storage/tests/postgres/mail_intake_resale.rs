//! 已交付装备的邮件再投稿必须在同一 intake 事务中复用旧 equipment_id 并创建新 lot。

#![cfg(feature = "test-db")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::time::Duration;

use auction_application::core::{
    ApplicationError, CompetitionTransactionError, RequestHash, TransactionPolicy,
    TransactionRunner,
};
use auction_application::mail::{
    MailAuctionIntakeTransaction, MailAuctionIntakeWrite, MailAuctionItemWrite,
};
use auction_domain::{CommandId, EquipmentId, EquipmentKeyEnvelope};
use chrono::{Duration as ChronoDuration, Utc};
use serde_json::json;
use uuid::Uuid;

#[allow(dead_code)]
use crate::support;

fn policy_snapshot(now: chrono::DateTime<Utc>) -> serde_json::Value {
    json!({
        "schema_version": 5,
        "start_price": 50000,
        "bid_increment_ppm": 100_000,
        "minimum_increment": 100,
        "soft_close_window_seconds": 900,
        "soft_close_extension_seconds": 900,
        "proxy_enabled": true,
        "proxy_default_mode": "fast",
        "proxy_slow_interval_seconds": 600,
        "proxy_max_outbid_uses": 3,
        "final_phase_lead_seconds": 900,
        "scheduled_end_at": now,
        "settlement": {
            "version": 2,
            "mode": "hold_curve_v2",
            "compensation_pool_enabled": true,
            "anchors": []
        },
        "recovery": {"version": 2, "payment_timeout_seconds": 86400, "mode": "remind_v1"},
        "bidding_version": 2
    })
}

#[tokio::test]
async fn postgres_mail_intake_reuses_delivered_equipment_for_resale() {
    let database = support::TestDatabase::create().await.expect("clone template db");
    let store = database.store().await.expect("connect test db");
    let pool = store.postgres_pool();
    let now = Utc::now();
    let old_auction = Uuid::new_v4();
    let old_lot = Uuid::new_v4();
    let old_command = Uuid::new_v4();
    let equipment = Uuid::new_v4();
    let winner = Uuid::new_v4();
    let delivery = Uuid::new_v4();
    let target_auction = Uuid::new_v4();
    let target_command = Uuid::new_v4();
    let fingerprint = "resale-fingerprint";
    let policy = policy_snapshot(now);

    sqlx::query(
        "INSERT INTO public.users (id, username, public_key_hash)
         VALUES ($1, 'armande1', $2)",
    )
    .bind(winner)
    .bind([8_u8; 32].as_slice())
    .execute(pool)
    .await
    .expect("winner");
    sqlx::query(
        "INSERT INTO public.equipment
            (id, world_id, external_equipment_id, name, category, attributes, collected_at)
         VALUES ($1, 'main', '316457702', 'Legendary Ethereal Great Mace of the Vampire',
                 'Two-Handed', $2, $3)",
    )
    .bind(equipment)
    .bind(json!({"type":"Two-Handed","quality":"Legendary","prefix":"Ethereal",
                 "suffix":"of the Vampire","tradeable":true,"level":100,"stats":{}}))
    .bind(now - ChronoDuration::days(20))
    .execute(pool)
    .await
    .expect("equipment");
    sqlx::query(
        "INSERT INTO public.equipment_secrets
            (equipment_id, ciphertext, nonce, encryption_key_id, fingerprint)
         VALUES ($1, $2, $3, 'test-key', $4)",
    )
    .bind(equipment)
    .bind(vec![1_u8; 32])
    .bind(vec![0_u8; 12])
    .bind(fingerprint)
    .execute(pool)
    .await
    .expect("equipment secret");
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, preview_opened_at, opened_at, policy_snapshot)
         VALUES ($1, 'main', 1, 'old resale auction', 'settled', $2, $2, $2, $2, $2, $2, $3)",
    )
    .bind(old_auction)
    .bind(now - ChronoDuration::days(20))
    .bind(&policy)
    .execute(pool)
    .await
    .expect("old auction");
    sqlx::query(
        "INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code)
         VALUES ($1, 'settlement-test', $2, $3, 'accepted')",
    )
    .bind(old_command)
    .bind(old_auction)
    .bind([1_u8; 32].as_slice())
    .execute(pool)
    .await
    .expect("old command");
    sqlx::query(
        "INSERT INTO public.lots
            (id, auction_id, equipment_id, lot_number, status, start_price, current_price,
             leader_user_id, effective_end_at, category_key, category_number)
         VALUES ($1, $2, $3, 1, 'sold', 50000, 50000, $4, $5,  'Two-Handed', 1)",
    )
    .bind(old_lot)
    .bind(old_auction)
    .bind(equipment)
    .bind(winner)
    .bind(now - ChronoDuration::days(20))
    .execute(pool)
    .await
    .expect("old lot");
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, completed_at, external_reference)
         VALUES ($1, $2, $3, 'mail_intake', 'old-delivery', 'armande1', '{}'::jsonb,
                 'succeeded', $4, 'old-mid')",
    )
    .bind(delivery)
    .bind(Uuid::new_v4())
    .bind([2_u8; 32].as_slice())
    .bind(now - ChronoDuration::days(20))
    .execute(pool)
    .await
    .expect("old delivery");
    sqlx::query(
        "INSERT INTO public.settlement_obligations
            (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
             obligation_kind, business_state, recipient_user_id, amount, carrier_credit,
             equipment_id)
         VALUES ($1, $2, $3, 'main', $4, 1, 'winner_equipment_cod', 'ready', $5, 50000, 0, $6)",
    )
    .bind(Uuid::new_v4())
    .bind(old_auction)
    .bind(old_lot)
    .bind(old_command)
    .bind(winner)
    .bind(equipment)
    .execute(pool)
    .await
    .expect("old obligation");
    let old_obligation: Uuid = sqlx::query_scalar(
        "SELECT id FROM public.settlement_obligations WHERE lot_id=$1 AND obligation_kind='winner_equipment_cod'",
    )
    .bind(old_lot)
    .fetch_one(pool)
    .await
    .expect("read old obligation");
    sqlx::query(
        "INSERT INTO public.settlement_obligation_fulfillments
            (obligation_id, execution_state, dispatch_world, delivery_operation, delivery_id, completed_at)
         VALUES ($1, 'satisfied', 'main', 'send', $2, $3)",
    )
    .bind(old_obligation)
    .bind(delivery)
    .bind(now - ChronoDuration::days(20))
    .execute(pool)
    .await
    .expect("old fulfillment");

    let mut target_tx = pool.begin().await.expect("target tx");
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, preview_opened_at, opened_at, policy_snapshot)
         VALUES ($1, 'main', 2, 'new resale target', 'draft', $2, $2, $2, $2, $2, $2, $3)",
    )
    .bind(target_auction)
    .bind(now)
    .bind(&policy)
    .execute(&mut *target_tx)
    .await
    .expect("target auction");
    sqlx::query(
        "INSERT INTO public.auction_intake_targets (world_id, auction_id, target_status)
         VALUES ('main', $1, 'draft')",
    )
    .bind(target_auction)
    .execute(&mut *target_tx)
    .await
    .expect("target row");
    target_tx.commit().await.expect("target commit");

    sqlx::query(
        "INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code)
         VALUES ($1, 'mail_auction_intake', $2, $3, 'pending')",
    )
    .bind(target_command)
    .bind(old_auction)
    .bind([3_u8; 32].as_slice())
    .execute(pool)
    .await
    .expect("intake command placeholder");
    // 删除占位行，让 owner 事务走自己的命令账本写入。
    sqlx::query("DELETE FROM public.auction_commands WHERE command_id=$1")
        .bind(target_command)
        .execute(pool)
        .await
        .expect("remove placeholder");

    let write = MailAuctionIntakeWrite {
        command_id: CommandId::from_uuid(target_command),
        aggregate_id: Uuid::new_v4(),
        world_id: "main".to_owned(),
        mailbox_key: "mooglemail:main".to_owned(),
        external_mail_id: "3126617".to_owned(),
        seller: "armande1".to_owned(),
        received_at: now,
        now,
        submission_id: Uuid::new_v4(),
        request_hash: RequestHash::new([4; 32]),
        items: vec![MailAuctionItemWrite {
            equipment_id: EquipmentId::from_uuid(Uuid::new_v4()),
            lot_id: Uuid::new_v4(),
            external_equipment_id: "316457702".to_owned(),
            name: "Legendary Ethereal Great Mace of the Vampire".to_owned(),
            category: "Two-Handed".to_owned(),
            level: Some(100),
            attributes: json!({"type":"Two-Handed","quality":"Legendary","prefix":"Ethereal",
                               "suffix":"of the Vampire","tradeable":true,"level":100,"stats":{}}),
            collected_at: now,
            start_price: 50000,
            key_envelope: EquipmentKeyEnvelope::new(
                vec![1_u8; 32],
                vec![0_u8; 12],
                "test-key".to_owned(),
                fingerprint.to_owned(),
            )
            .expect("key envelope"),
        }],
    };
    let pg = &store;
    let result = pg
        .run(TransactionPolicy::serializable(), |tx| {
            let write = write.clone();
            Box::pin(async move {
                let result =
                    tx.add_mail_auction_intake(write).await.map_err(ApplicationError::from)?;
                Ok::<_, ApplicationError>(result)
            })
        })
        .await
        .expect("resale intake must commit");
    assert_eq!(result.added_count, 1);
    let new_lot: (Uuid, Uuid, Uuid, String) = sqlx::query_as(
        "SELECT lot.id, lot.auction_id, lot.equipment_id, lot.status
         FROM public.lots AS lot
         WHERE lot.id = $1",
    )
    .bind(result.lot_ids[0])
    .fetch_one(pool)
    .await
    .expect("new lot");
    assert_eq!(new_lot.1, target_auction);
    assert_eq!(new_lot.2, equipment, "resale must reuse old equipment id");
    assert_eq!(new_lot.3, "pending");
    let old_still_sold: String = sqlx::query_scalar("SELECT status FROM public.lots WHERE id=$1")
        .bind(old_lot)
        .fetch_one(pool)
        .await
        .expect("old lot");
    assert_eq!(old_still_sold, "sold");
    let _ = (target_command, CompetitionTransactionError::Conflict, Duration::from_secs(0));
    store.postgres_pool().close().await;
}
