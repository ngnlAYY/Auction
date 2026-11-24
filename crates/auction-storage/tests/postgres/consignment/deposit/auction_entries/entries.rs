//! 邮件投稿与 Admin 录入的寄售强占用判定、别名与锁键身份回归。

use std::assert_matches;

use auction_application::core::{ApplicationError, CompetitionTransactionError, RequestHash};
use auction_application::mail::MailAuctionIntakeResult;
use auction_domain::consignment::ListingEquipmentIdentity;
use auction_time::Timestamp;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::support::execute;
use super::fixtures::{
    admin_command, admin_service, count_lots, equipment_key, hold_equipment_lock, mail_command,
    mail_service, seed_draft_target, seed_position_claim, seed_reservation_claim,
    seed_reservation_fixture,
};
use crate::support::TestDatabase;

async fn assert_no_entry_facts(pool: &PgPool, auction_id: Uuid) {
    assert_eq!(count_lots(pool, auction_id).await, 0, "不得新增 lot");
    let submissions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.auction_submissions WHERE auction_id=$1")
            .bind(auction_id)
            .fetch_one(pool)
            .await
            .expect("读取投稿数");
    assert_eq!(submissions, 0, "不得新增投稿");
    let return_intents: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.deliveries
         WHERE origin_kind = 'mail_intake' AND payload_operation = 'return'",
    )
    .fetch_one(pool)
    .await
    .expect("读取退回意图");
    assert_eq!(return_intents, 0, "不得安排退回");
}

async fn claim_holders(pool: &PgPool) -> (Option<Uuid>, Option<Uuid>) {
    sqlx::query_as(
        "SELECT reservation_id, position_id FROM public.consignment_equipment_claims
         WHERE world_id='main' AND canonical_eid='42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取强占用")
}

#[tokio::test]
async fn postgres_mail_entry_reservation_claim_is_held_without_facts() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let auction_id = seed_draft_target(pool, "main", 10_000).await;
    let reservation = seed_reservation_claim(pool).await;
    let now = Timestamp::now();
    let result =
        mail_service(store.clone(), now).ingest_mail(mail_command("main", &["42"], 1, now)).await;
    assert_matches!(
        result,
        Err(ApplicationError::CompetitionTransaction(
            auction_application::core::CompetitionTransactionError::EquipmentHeld
        ))
    );
    assert_no_entry_facts(pool, auction_id).await;
    assert_eq!(claim_holders(pool).await, (Some(reservation), None));
    let state: String =
        sqlx::query_scalar("SELECT state FROM public.consignment_deposit_reservations WHERE id=$1")
            .bind(reservation)
            .fetch_one(pool)
            .await
            .expect("读取预约");
    assert_eq!(state, "active", "预约不得被拍卖入站改动");
}

#[tokio::test]
async fn postgres_mail_entry_existing_submission_conflict_precedes_strong_claim_without_lock() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let auction_id = seed_draft_target(pool, "main", 10_009).await;
    let reservation = seed_reservation_claim(pool).await;
    execute(
        pool,
        &format!(
            "INSERT INTO public.auction_commands
                (command_id, command_kind, aggregate_id, request_hash, result_code)
             VALUES (gen_random_uuid(), 'existing_submission', '{auction_id}',
                     decode(repeat('31', 32), 'hex'), 'accepted');
             INSERT INTO public.auction_submissions
                (id, source_command_id, auction_id, mailbox_key, external_mail_id, seller,
                 received_at, source_kind)
             SELECT gen_random_uuid(), command_id, '{auction_id}', 'mooglemail:main', '7000001',
                    'FakeSellerOne', clock_timestamp(), 'mail'
             FROM public.auction_commands
             WHERE command_kind='existing_submission' AND aggregate_id='{auction_id}';"
        ),
    )
    .await
    .expect("写入既有投稿冲突");
    let identity = ListingEquipmentIdentity::new("main", "42").expect("固定身份");
    let _holder = hold_equipment_lock(pool, equipment_key(&identity)).await;
    let now = Timestamp::now();
    let result =
        mail_service(store.clone(), now).ingest_mail(mail_command("main", &["42"], 1, now)).await;
    assert_matches!(
        result,
        Err(ApplicationError::CompetitionTransaction(CompetitionTransactionError::Conflict)),
        "勘误 E1：既有投稿续办必须先返回原 Conflict，且不得尝试实物锁"
    );
    assert_eq!(count_lots(pool, auction_id).await, 0, "续办失败方零新 lot");
    let submissions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.auction_submissions WHERE auction_id=$1")
            .bind(auction_id)
            .fetch_one(pool)
            .await
            .expect("读取投稿数");
    assert_eq!(submissions, 1, "既有投稿事实不得被改动");
    assert_eq!(claim_holders(pool).await, (Some(reservation), None));
}

#[tokio::test]
async fn postgres_mail_entry_strong_claim_precedes_existing_equipment_conflict() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let auction_id = seed_draft_target(pool, "main", 10_015).await;
    let reservation = seed_reservation_claim(pool).await;
    // 已登记但无密钥信封的装备会走既有 AdminIntakeEquipmentConflict；强占用判定必须先返回
    // EquipmentHeld，证明锁内显式检查不是 lot 守卫的重复实现。
    sqlx::query(
        "INSERT INTO public.equipment
            (id, world_id, external_equipment_id, name, category, attributes, collected_at)
         VALUES (gen_random_uuid(), 'main', '42', 'Fake Existing Gear', 'Weapon',
                 '{}'::jsonb, clock_timestamp())",
    )
    .execute(pool)
    .await
    .expect("写入既有装备");
    let now = Timestamp::now();
    let result =
        mail_service(store.clone(), now).ingest_mail(mail_command("main", &["42"], 7, now)).await;
    assert_matches!(
        result,
        Err(ApplicationError::CompetitionTransaction(CompetitionTransactionError::EquipmentHeld)),
        "强占用必须先于既有装备冲突返回"
    );
    assert_no_entry_facts(pool, auction_id).await;
    assert_eq!(claim_holders(pool).await, (Some(reservation), None));
}

#[tokio::test]
async fn postgres_mail_entry_replay_skips_equipment_lock_when_claim_held() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let auction_id = seed_draft_target(pool, "main", 10_016).await;
    seed_reservation_claim(pool).await;
    let command_id = Uuid::new_v4();
    let request_hash = RequestHash::new([21; 32]);
    let expected = MailAuctionIntakeResult {
        auction_id,
        auction_sequence: 1,
        auction_state_version: 1,
        preview_revision: 0,
        submission_id: Uuid::new_v4(),
        added_count: 1,
        existing_count: 0,
        lot_ids: vec![Uuid::new_v4()],
    };
    sqlx::query(
        "INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code, result_payload)
         VALUES ($1, 'mail_auction_intake', $2, $3, 'accepted', $4)",
    )
    .bind(command_id)
    .bind(auction_application::mail::mail_auction_aggregate_id("main"))
    .bind(request_hash.as_bytes().as_slice())
    .bind(serde_json::to_value(&expected).expect("结果编码"))
    .execute(pool)
    .await
    .expect("写入已接受命令");
    let identity = ListingEquipmentIdentity::new("main", "42").expect("固定身份");
    let _holder = hold_equipment_lock(pool, equipment_key(&identity)).await;

    let now = Timestamp::now();
    let mut command = mail_command("main", &["42"], 8, now);
    command.command_id = auction_domain::CommandId::from_uuid(command_id);
    command.request_hash = request_hash;
    let result = mail_service(store.clone(), now).ingest_mail(command).await.expect("重放成功");
    assert_eq!(result.auction_id, expected.auction_id);
    assert_eq!(result.lot_ids, expected.lot_ids);
}

#[tokio::test]
async fn postgres_admin_entry_reservation_claim_maps_to_existing_conflict() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let auction_id = seed_draft_target(pool, "main", 10_001).await;
    let reservation = seed_reservation_claim(pool).await;
    let now = Timestamp::now();
    let result =
        admin_service(store.clone(), "42", now).add_item(admin_command("main", "42", now)).await;
    assert_matches!(
        result,
        Err(ApplicationError::CompetitionTransaction(
            auction_application::core::CompetitionTransactionError::AdminIntakeEquipmentConflict
        ))
    );
    assert_no_entry_facts(pool, auction_id).await;
    assert_eq!(claim_holders(pool).await, (Some(reservation), None));
}

#[tokio::test]
async fn postgres_mail_entry_position_claim_is_held_without_facts() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let auction_id = seed_draft_target(pool, "main", 10_002).await;
    let position = seed_position_claim(&database).await;
    let now = Timestamp::now();
    let result =
        mail_service(store.clone(), now).ingest_mail(mail_command("main", &["42"], 2, now)).await;
    assert_matches!(
        result,
        Err(ApplicationError::CompetitionTransaction(
            auction_application::core::CompetitionTransactionError::EquipmentHeld
        ))
    );
    assert_no_entry_facts(pool, auction_id).await;
    assert_eq!(claim_holders(pool).await, (None, Some(position)));
}

#[tokio::test]
async fn postgres_admin_entry_position_claim_maps_to_existing_conflict() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let auction_id = seed_draft_target(pool, "main", 10_003).await;
    let position = seed_position_claim(&database).await;
    let now = Timestamp::now();
    let result =
        admin_service(store.clone(), "42", now).add_item(admin_command("main", "42", now)).await;
    assert_matches!(
        result,
        Err(ApplicationError::CompetitionTransaction(
            auction_application::core::CompetitionTransactionError::AdminIntakeEquipmentConflict
        ))
    );
    assert_no_entry_facts(pool, auction_id).await;
    assert_eq!(claim_holders(pool).await, (None, Some(position)));
}

#[tokio::test]
async fn postgres_mail_entry_padded_alias_shares_the_same_equipment_lock() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let auction_id = seed_draft_target(pool, "main", 10_004).await;
    let reservation = seed_reservation_claim(pool).await;
    let now = Timestamp::now();
    let result = mail_service(store.clone(), now)
        .ingest_mail(mail_command("main", &["00042"], 3, now))
        .await;
    assert_matches!(
        result,
        Err(ApplicationError::CompetitionTransaction(
            auction_application::core::CompetitionTransactionError::EquipmentHeld
        ))
    );
    assert_no_entry_facts(pool, auction_id).await;
    assert_eq!(claim_holders(pool).await, (Some(reservation), None));
}

#[tokio::test]
async fn postgres_admin_entry_padded_alias_shares_the_same_equipment_lock() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let auction_id = seed_draft_target(pool, "main", 10_014).await;
    let reservation = seed_reservation_claim(pool).await;
    let now = Timestamp::now();
    let result = admin_service(store.clone(), "00042", now)
        .add_item(admin_command("main", "00042", now))
        .await;
    assert_matches!(
        result,
        Err(ApplicationError::CompetitionTransaction(
            auction_application::core::CompetitionTransactionError::AdminIntakeEquipmentConflict
        ))
    );
    assert_no_entry_facts(pool, auction_id).await;
    assert_eq!(claim_holders(pool).await, (Some(reservation), None));
}

#[tokio::test]
async fn postgres_mail_entry_same_eid_in_another_world_is_not_held() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let auction_id = seed_draft_target(pool, "isekai", 10_005).await;
    seed_reservation_claim(pool).await;
    let now = Timestamp::now();
    let result = mail_service(store.clone(), now)
        .ingest_mail(mail_command("isekai", &["42"], 4, now))
        .await
        .expect("另一世界不受主世界强占用影响");
    assert_eq!(result.added_count, 1);
    assert_eq!(count_lots(pool, auction_id).await, 1);
}

#[tokio::test]
async fn postgres_mail_entry_unnormalizable_eid_skips_the_lock_and_keeps_old_behavior() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let auction_id = seed_draft_target(pool, "main", 10_006).await;
    seed_reservation_fixture(pool).await;
    let now = Timestamp::now();
    let result = mail_service(store.clone(), now)
        .ingest_mail(mail_command("main", &["000"], 5, now))
        .await
        .expect("无法规范化的旧 EID 不取锁，按改动前继续");
    assert_eq!(result.added_count, 1);
    assert_eq!(count_lots(pool, auction_id).await, 1);
    let stored: String = sqlx::query_scalar(
        "SELECT external_equipment_id FROM public.equipment
         WHERE world_id='main' AND external_equipment_id='000'",
    )
    .fetch_one(pool)
    .await
    .expect("原文 EID 保持原样");
    assert_eq!(stored, "000");
}

#[tokio::test]
async fn postgres_mail_entry_without_claim_matches_previous_result() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let auction_id = seed_draft_target(pool, "main", 10_007).await;
    seed_reservation_fixture(pool).await;
    let now = Timestamp::now();
    let result = mail_service(store.clone(), now)
        .ingest_mail(mail_command("main", &["42"], 6, now))
        .await
        .expect("无强占用时照常入站");
    assert_eq!(result.auction_id, auction_id);
    assert_eq!(result.added_count, 1);
    assert_eq!(count_lots(pool, auction_id).await, 1);
}

#[tokio::test]
async fn postgres_admin_entry_without_claim_matches_previous_result() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let auction_id = seed_draft_target(pool, "main", 10_008).await;
    seed_reservation_fixture(pool).await;
    let now = Timestamp::now();
    let result = admin_service(store.clone(), "42", now)
        .add_item(admin_command("main", "42", now))
        .await
        .expect("无强占用时照常录入");
    assert_eq!(result.auction_id, auction_id);
    assert_eq!(count_lots(pool, auction_id).await, 1);
}
