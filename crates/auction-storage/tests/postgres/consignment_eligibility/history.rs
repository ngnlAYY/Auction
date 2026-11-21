//! 以拍卖原始行验证历史证据与领域拒绝原因。

use auction_domain::UserId;
use auction_domain::consignment::{ListingError, check_listing_history};
use sqlx::PgConnection;
use uuid::Uuid;

use super::fixtures::{
    END, LATER, OTHER, SELLER, START, auction, batch, commit, equip, gear, lot, seed, submission,
};
use super::production::history::read_listing_history;
use crate::support::TestDatabase;

async fn verdict(cx: &mut PgConnection, eid: &str, user: Uuid) -> Result<(), ListingError> {
    let identity = gear("main", eid);
    let evidence = read_listing_history(cx, &identity).await.expect("历史证据读取成功");
    check_listing_history(&identity, UserId::from_uuid(user), &evidence.aliases)
}

struct ReturnEvidence {
    lot_id: Uuid,
    auction_id: Uuid,
    equipment_id: Uuid,
    submission_id: Uuid,
    kind: &'static str,
    recipient: &'static str,
    collected: bool,
}

impl ReturnEvidence {
    async fn record(&self, tx: &mut PgConnection) -> (Uuid, Uuid) {
        let Self { lot_id, auction_id, equipment_id, submission_id, kind, recipient, collected } =
            self;
        let obligation_id = Uuid::new_v4();
        let delivery_id = Uuid::new_v4();
        let command_id = Uuid::new_v4();
        let delivery_command_id = Uuid::new_v4();
        let (state, proof) = if *collected {
            (
                "satisfied",
                format!(
                    "'{END}','mooglemail_sent_collected','12345',decode(repeat('11',32),'hex'),'{END}'"
                ),
            )
        } else {
            ("enqueued", "NULL,NULL,NULL,NULL,NULL".into())
        };
        batch(tx, &format!(r#"
            INSERT INTO public.auction_commands (command_id,command_kind,aggregate_id,request_hash,result_code)
            VALUES ('{command_id}','fake_settlement','{auction_id}',decode(repeat('11',32),'hex'),'accepted');
            INSERT INTO public.settlement_obligations
                (id,auction_id,lot_id,world_id,origin_command_id,policy_version,obligation_kind,
                 business_state,recipient_game_identity,equipment_id,submission_id)
            VALUES ('{obligation_id}','{auction_id}','{lot_id}','main','{command_id}',1,'{kind}',
                    'ready','{recipient}','{equipment_id}','{submission_id}');
            INSERT INTO public.deliveries
                (id,command_id,request_digest,origin_kind,origin_id,recipient,payload,status,
                 external_reference,result_metadata,completed_at)
            VALUES ('{delivery_id}','{delivery_command_id}',decode(repeat('11',32),'hex'),
                    'settlement','{obligation_id}','{recipient}',
                    '{{"schema_version":1,"world":"main","operation":"send",
                       "equipment_secret_refs":[{{"kind":"auction_equipment_key","id":"{equipment_id}"}}]}}',
                    'succeeded','mooglemail:main:send:12345',
                    '{{"external_reference_proven":true,"operation":"send","proof":"sent_exact_payload_v1"}}',
                    '{LATER}');
            INSERT INTO public.settlement_obligation_fulfillments
                (obligation_id,execution_state,dispatch_world,delivery_operation,delivery_id,
                 completed_at,proof_kind,proof_reference,proof_digest,proof_observed_at)
            VALUES ('{obligation_id}','{state}','main','send','{delivery_id}',{proof});
        "#)).await;
        (obligation_id, delivery_id)
    }
}

async fn record_return(
    tx: &mut PgConnection,
    auction_id: Uuid,
    eid: &str,
    seller: &'static str,
    status: &str,
    collected: bool,
) -> ReturnEvidence {
    let equipment_id = equip(tx, "main", eid).await;
    let submission_id = submission(tx, auction_id, "admin", seller, START).await;
    let lot_id = Uuid::new_v4();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM public.lots WHERE auction_id=$1")
        .bind(auction_id)
        .fetch_one(&mut *tx)
        .await
        .expect("场次 lot 数");
    lot(
        tx,
        lot_id,
        auction_id,
        equipment_id,
        i32::try_from(count + 1).expect("测试 lot 号"),
        status,
        Some(submission_id),
    )
    .await;
    let kind = if status == "unsold" { "unsold_return" } else { "withdrawn_return" };
    ReturnEvidence {
        lot_id,
        auction_id,
        equipment_id,
        submission_id,
        kind,
        recipient: seller,
        collected,
    }
}

#[tokio::test]
async fn domain_history_requires_matching_collection_and_stable_seller() {
    let db = TestDatabase::create().await.expect("临时 PG18");
    let owner = db.maintenance_store().await.expect("维护连接");
    let runtime = db.store().await.expect("服务连接");
    let pool = owner.postgres_pool();
    seed(pool).await;
    let mut tx = pool.begin().await.expect("历史夹具");
    let settled = auction(&mut tx, "main", 1, "settled").await;
    let draft = auction(&mut tx, "main", 2, "draft").await;
    let proved = record_return(&mut tx, settled, "00042", "FakeOldSeller", "unsold", true).await;
    batch(
        &mut tx,
        &format!(
            "INSERT INTO public.user_renames
        (id,user_id,previous_username,next_username,actor,created_at)
        VALUES (gen_random_uuid(),'{SELLER}','FakeOldSeller','FakeSeller','fake-actor','{END}');"
        ),
    )
    .await;
    proved.record(&mut tx).await;
    record_return(&mut tx, settled, "43", "FakeSeller", "unsold", false)
        .await
        .record(&mut tx)
        .await;
    record_return(&mut tx, draft, "44", "FakeSeller", "withdrawn", true)
        .await
        .record(&mut tx)
        .await;
    record_return(&mut tx, draft, "47", "FakeSeller", "withdrawn", true).await;
    let reserved = equip(&mut tx, "main", "45").await;
    lot(&mut tx, Uuid::new_v4(), draft, reserved, 3, "pending", None).await;
    equip(&mut tx, "main", "46").await;
    commit(tx).await;

    let mut read = runtime.postgres_pool().acquire().await.expect("历史连接");
    for (eid, user, expected) in [
        ("41", SELLER, Ok(())),
        ("42", SELLER, Ok(())),
        ("42", OTHER, Err(ListingError::HistoryOwnerMismatch)),
        ("43", SELLER, Err(ListingError::EquipmentBusy)),
        ("44", SELLER, Ok(())),
        ("44", OTHER, Err(ListingError::HistoryOwnerMismatch)),
        ("45", SELLER, Err(ListingError::EquipmentBusy)),
        ("46", SELLER, Err(ListingError::HistoryUnproven)),
        ("47", SELLER, Err(ListingError::HistoryUnproven)),
    ] {
        assert_eq!(verdict(&mut read, eid, user).await, expected, "测试装备 {eid}");
    }
}

#[tokio::test]
async fn domain_history_preserves_alias_disagreement_and_checks_delivery_binding() {
    let db = TestDatabase::create().await.expect("临时 PG18");
    let owner = db.maintenance_store().await.expect("维护连接");
    let runtime = db.store().await.expect("服务连接");
    let pool = owner.postgres_pool();
    seed(pool).await;
    let mut tx = pool.begin().await.expect("历史夹具");
    let settled = auction(&mut tx, "main", 1, "settled").await;
    record_return(&mut tx, settled, "42", "FakeSeller", "unsold", true).await.record(&mut tx).await;
    record_return(&mut tx, settled, "00042", "FakeOther", "unsold", true)
        .await
        .record(&mut tx)
        .await;
    let (_, wrong_reference) = record_return(&mut tx, settled, "43", "FakeSeller", "unsold", true)
        .await
        .record(&mut tx)
        .await;
    let (_, wrong_attachment) = record_return(&mut tx, settled, "44", "FakeSeller", "unsold", true)
        .await
        .record(&mut tx)
        .await;
    let (_, wrong_recipient) = record_return(&mut tx, settled, "45", "FakeSeller", "unsold", true)
        .await
        .record(&mut tx)
        .await;
    let alias_a = record_return(&mut tx, settled, "46", "FakeSeller", "unsold", true).await;
    let alias_b = record_return(&mut tx, settled, "00046", "FakeSeller", "unsold", true).await;
    alias_a.record(&mut tx).await;
    alias_b.record(&mut tx).await;
    batch(&mut tx, &format!(r#"
        UPDATE public.deliveries SET external_reference='mooglemail:main:send:54321' WHERE id='{wrong_reference}';
        UPDATE public.deliveries SET payload=jsonb_set(payload,'{{equipment_secret_refs}}','[]')
        WHERE id='{wrong_attachment}';
        UPDATE public.deliveries SET recipient='FakeOther' WHERE id='{wrong_recipient}';
        INSERT INTO public.equipment_secrets(equipment_id,ciphertext,nonce,encryption_key_id,fingerprint)
        VALUES ('{}',decode('11','hex'),decode(repeat('11',12),'hex'),'fake-generation',repeat('a',64)),
               ('{}',decode('11','hex'),decode(repeat('11',12),'hex'),'fake-generation',repeat('b',64));
    "#, alias_a.equipment_id, alias_b.equipment_id)).await;
    commit(tx).await;

    let mut read = runtime.postgres_pool().acquire().await.expect("历史连接");
    assert_eq!(verdict(&mut read, "42", SELLER).await, Err(ListingError::HistoryAliasConflict));
    assert_eq!(verdict(&mut read, "42", OTHER).await, Err(ListingError::HistoryAliasConflict));
    for eid in ["43", "44", "45"] {
        assert_eq!(
            verdict(&mut read, eid, SELLER).await,
            Err(ListingError::EquipmentBusy),
            "测试装备 {eid}"
        );
    }
    assert_eq!(verdict(&mut read, "46", SELLER).await, Err(ListingError::HistoryAliasConflict));
}

#[tokio::test]
async fn domain_history_follows_proven_alias_handoff() {
    let db = TestDatabase::create().await.expect("临时 PG18");
    let owner = db.maintenance_store().await.expect("维护连接");
    let runtime = db.store().await.expect("服务连接");
    let pool = owner.postgres_pool();
    seed(pool).await;
    let mut tx = pool.begin().await.expect("交接夹具");
    let first_auction = auction(&mut tx, "main", 1, "settled").await;
    let second_auction = auction(&mut tx, "main", 2, "settled").await;
    let first = record_return(&mut tx, first_auction, "42", "FakeSeller", "unsold", true).await;
    let second =
        record_return(&mut tx, second_auction, "00042", "FakeSeller", "unsold", true).await;
    first.record(&mut tx).await;
    let (second_obligation, second_delivery) = second.record(&mut tx).await;
    batch(&mut tx, &format!(r#"
        UPDATE public.auction_submissions SET received_at='2026-01-03T00:00:00Z' WHERE id='{}';
        UPDATE public.lots SET status='sold', leader_user_id='{OTHER}', current_price=50000,
            effective_end_at='2026-01-03T02:00:00Z',
            created_at='2026-01-03T01:00:00Z',updated_at='2026-01-03T01:00:00Z' WHERE id='{}';
        UPDATE public.settlement_obligations SET obligation_kind='winner_equipment_cod',
            recipient_game_identity=NULL,recipient_user_id='{OTHER}',amount=50000 WHERE id='{second_obligation}';
        UPDATE public.deliveries SET recipient='FakeOther',completed_at='2026-01-04T00:00:00Z'
            WHERE id='{second_delivery}';
        UPDATE public.settlement_obligation_fulfillments SET completed_at='2026-01-05T00:00:00Z',
            proof_observed_at='2026-01-05T00:00:00Z' WHERE obligation_id='{second_obligation}';
        INSERT INTO public.equipment_secrets(equipment_id,ciphertext,nonce,encryption_key_id,fingerprint)
        SELECT e.id, decode('11','hex'),decode(repeat('11',12),'hex'),'fake-generation',repeat('a',64)
        FROM public.equipment e WHERE e.id IN ('{}','{}');
    "#, second.submission_id, second.lot_id, first.equipment_id, second.equipment_id)).await;
    commit(tx).await;

    let mut read = runtime.postgres_pool().acquire().await.expect("交接读取");
    assert_eq!(verdict(&mut read, "42", OTHER).await, Ok(()));
    assert_eq!(verdict(&mut read, "42", SELLER).await, Err(ListingError::HistoryOwnerMismatch));
    let mut admin = pool.acquire().await.expect("变化证据");
    batch(
        &mut admin,
        &format!(
            "UPDATE public.equipment_secrets SET encryption_key_id='fake-new-generation'
         WHERE equipment_id='{}';",
            second.equipment_id
        ),
    )
    .await;
    assert_eq!(verdict(&mut read, "42", OTHER).await, Err(ListingError::HistoryAliasConflict));

    let mut tx = pool.begin().await.expect("原别名行上的第三笔转手");
    let third_auction = auction(&mut tx, "main", 3, "settled").await;
    let third_submission =
        submission(&mut tx, third_auction, "admin", "FakeOther", "2026-01-06T00:00:00Z").await;
    let third_lot = Uuid::new_v4();
    lot(&mut tx, third_lot, third_auction, first.equipment_id, 1, "unsold", Some(third_submission))
        .await;
    let third = ReturnEvidence {
        lot_id: third_lot,
        auction_id: third_auction,
        equipment_id: first.equipment_id,
        submission_id: third_submission,
        kind: "unsold_return",
        recipient: "FakeOther",
        collected: true,
    };
    let (third_obligation, third_delivery) = third.record(&mut tx).await;
    batch(&mut tx, &format!(
        "UPDATE public.equipment_secrets SET encryption_key_id='fake-generation' WHERE equipment_id='{}';
         UPDATE public.lots SET created_at='2026-01-06T01:00:00Z',updated_at='2026-01-06T01:00:00Z'
             WHERE id='{third_lot}';
         UPDATE public.deliveries SET completed_at='2026-01-07T00:00:00Z' WHERE id='{third_delivery}';
         UPDATE public.settlement_obligation_fulfillments SET completed_at='2026-01-08T00:00:00Z',
             proof_observed_at='2026-01-08T00:00:00Z' WHERE obligation_id='{third_obligation}';",
        second.equipment_id
    )).await;
    commit(tx).await;
    assert_eq!(verdict(&mut read, "42", OTHER).await, Ok(()));
}
