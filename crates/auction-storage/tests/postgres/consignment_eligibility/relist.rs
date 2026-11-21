//! 验证恢复事务复用投稿时，新的 lot 仍立即成为碰撞。

use auction_application::recovery::RecoveryResolutionStore;
use auction_domain::UserId;
use auction_domain::consignment::{ListingError, check_listing_history};
use auction_time::{Duration, Timestamp};
use uuid::Uuid;

use super::fixtures::{
    END, LATER, Listing, OTHER, SELLER, START, auction, batch, commit, equip, gear, lot, seed,
    submission,
};
use super::history::record_relisted_return;
use super::production::history::read_listing_history;
use super::production::lots::{
    insert_registration_baselines, listing_collision, unassociated_collision_lots,
};
use crate::support::TestDatabase;

#[tokio::test]
async fn recovery_relist_enters_difference_with_reused_submission() {
    let db = TestDatabase::create().await.expect("临时 PG18");
    let owner = db.maintenance_store().await.expect("维护连接");
    let runtime = db.store().await.expect("服务连接");
    let pool = owner.postgres_pool();
    let service = runtime.postgres_pool();
    seed(pool).await;
    let mut tx = pool.begin().await.expect("原成交及待重列事实");
    let old_auction = auction(&mut tx, "main", 1, "settled").await;
    let draft = auction(&mut tx, "main", 2, "draft").await;
    let equipment_id = equip(&mut tx, "main", "00042").await;
    let submission_id = submission(&mut tx, old_auction, "admin", "FakeSeller", START).await;
    let old_lot = Uuid::new_v4();
    lot(&mut tx, old_lot, old_auction, equipment_id, 1, "sold", Some(submission_id)).await;

    let recovery_id = Uuid::new_v4();
    let payment = Uuid::new_v4();
    let seller_payout = Uuid::new_v4();
    let refund = Uuid::new_v4();
    let command_id = Uuid::new_v4();
    let take_action = Uuid::new_v4();
    let take_delivery = Uuid::new_v4();
    let return_mail = Uuid::new_v4();
    batch(&mut tx, &format!(r#"
        UPDATE public.lots SET leader_user_id='{OTHER}', current_price=50000 WHERE id='{old_lot}';
        INSERT INTO public.auction_commands (command_id,command_kind,aggregate_id,request_hash,result_code)
        VALUES ('{command_id}','fake_settlement','{old_auction}',decode(repeat('11',32),'hex'),'accepted');
        INSERT INTO public.settlement_obligations
            (id,auction_id,lot_id,world_id,origin_command_id,policy_version,obligation_kind,
             business_state,recipient_user_id,recipient_game_identity,amount,equipment_id,submission_id,
             prerequisite_obligation_id)
        VALUES
            ('{payment}','{old_auction}','{old_lot}','main','{command_id}',1,'winner_equipment_cod',
             'ready','{OTHER}',NULL,50000,'{equipment_id}','{submission_id}',NULL),
            ('{seller_payout}','{old_auction}','{old_lot}','main','{command_id}',1,'seller_payout',
             'blocked',NULL,'FakeSeller',50000,'{equipment_id}','{submission_id}','{payment}'),
            ('{refund}','{old_auction}','{old_lot}','main','{command_id}',1,'buyer_refund',
             'contingent','{OTHER}',NULL,50000,'{equipment_id}','{submission_id}','{payment}');
        INSERT INTO public.inbound_mail
            (id,mailbox_key,external_mail_id,world_id,mail_kind,content_digest,
             received_at,status,decision_code,processed_at)
        VALUES ('{return_mail}','fake-mailbox','8765','main','delivery',decode(repeat('11',32),'hex'),
                '{LATER}','accepted','settlement_recovery_returned','{LATER}');
        INSERT INTO public.deliveries
            (id,command_id,request_digest,origin_kind,origin_id,recipient,payload,status,
             completed_at,external_reference)
        VALUES ('{take_delivery}',gen_random_uuid(),decode(repeat('11',32),'hex'),
                'settlement_recovery_action','{take_action}','FakeOther','{{"world":"main","operation":"take"}}',
                'succeeded','{LATER}','mooglemail:main:take:8765');
        INSERT INTO public.settlement_recoveries
            (id,auction_id,lot_id,world_id,equipment_id,winner_user_id,payment_obligation_id,
             seller_payout_obligation_id,buyer_refund_obligation_id,policy_version,payment_deadline_at,
             stage,resolution,return_mail_id,return_external_mail_id,cod_return_count,cod_return_limit,
             custody_proof_kind,custody_proof_reference,custody_proof_digest,custody_proof_observed_at,
             created_at,updated_at)
        VALUES ('{recovery_id}','{old_auction}','{old_lot}','main','{equipment_id}','{OTHER}','{payment}',
                '{seller_payout}','{refund}',1,'{END}','relist_pending','reclaimed','{return_mail}','8765',3,3,
                'delivery_recovery_take_succeeded','mooglemail:main:take:8765',decode(repeat('22',32),'hex'),
                '{LATER}','{START}','{LATER}');
        INSERT INTO public.settlement_recovery_actions
            (id,recovery_id,kind,ordinal,execution_state,source_mail_id,delivery_id,
             proof_kind,proof_reference,proof_digest,proof_observed_at,completed_at)
        VALUES
            ('{take_action}','{recovery_id}','reclaim_equipment_take',1,'satisfied','{return_mail}','{take_delivery}',
             'delivery_recovery_take_succeeded','mooglemail:main:take:8765',decode(repeat('22',32),'hex'),'{LATER}','{LATER}'),
            (gen_random_uuid(),'{recovery_id}','buyer_blacklist',2,'satisfied',NULL,NULL,
             'buyer_blacklisted','fake-blacklist',decode(repeat('33',32),'hex'),'{LATER}','{LATER}');
    "#)).await;
    commit(tx).await;

    let listing = Listing::new(101, "42");
    let mut tx = service.begin().await.expect("统一冻结登记基线");
    listing.register(&mut tx).await;
    assert_eq!(
        insert_registration_baselines(&mut tx, listing.listing_id).await.expect("原 lot 基线"),
        1
    );
    assert_eq!(
        listing_collision(&mut tx, listing.listing_id).await.expect("当时无碰撞"),
        Some(false)
    );
    commit(tx).await;

    let now: Timestamp = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(service)
        .await
        .expect("数据库时刻");
    let result = runtime
        .advance_due_relist(now, Duration::minutes(1))
        .await
        .expect("保持原来恢复入口")
        .expect("取得重列结果");
    assert_eq!(result.relisted_auction_id, Some(draft));
    let new_lot = result.relisted_lot_id.expect("已提交重列 lot");
    let new_submission: Uuid =
        sqlx::query_scalar("SELECT submission_id FROM public.lots WHERE id=$1")
            .bind(new_lot)
            .fetch_one(service)
            .await
            .expect("重列保留投稿");
    assert_eq!(new_submission, submission_id);

    let mut after = service.acquire().await.expect("重列后新快照");
    assert_eq!(
        unassociated_collision_lots(&mut after, listing.listing_id).await.expect("重列碰撞"),
        [new_lot]
    );

    let identity = gear("main", "42");
    let history = read_listing_history(&mut after, &identity).await.expect("仍在拍卖的重列历史");
    assert_eq!(
        check_listing_history(&identity, UserId::from_uuid(SELLER), &history.aliases),
        Err(ListingError::EquipmentBusy)
    );

    let mut tx = pool.begin().await.expect("重列后流拍并领取返还");
    batch(
        &mut tx,
        &format!(
            r#"
        DELETE FROM public.auction_intake_targets WHERE auction_id='{draft}';
        UPDATE public.auctions SET status='settled' WHERE id='{draft}';
        UPDATE public.lots SET status='unsold' WHERE id='{new_lot}';
    "#
        ),
    )
    .await;
    record_relisted_return(&mut tx, draft, new_lot, equipment_id, submission_id).await;
    commit(tx).await;

    let history = read_listing_history(&mut after, &identity).await.expect("已返还的重列历史");
    assert_eq!(
        check_listing_history(&identity, UserId::from_uuid(SELLER), &history.aliases),
        Err(ListingError::HistoryUnproven)
    );
}
