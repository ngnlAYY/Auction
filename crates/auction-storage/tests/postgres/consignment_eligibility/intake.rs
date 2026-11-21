//! 从现有 MoogleMail 接受事务与 Admin 录入事务生成真实碰撞事实。

use auction_application::admin::{
    AdminIntakeRecord, AdminIntakeSourceKind, AdminIntakeTransaction, AdminIntakeWrite,
};
use auction_application::core::{
    ApplicationError, RequestHash, TransactionPolicy, TransactionRunner,
};
use auction_application::mail::{
    MailAuctionIntakeRecord, MailAuctionIntakeTransaction, MailAuctionIntakeWrite,
    MailAuctionItemWrite,
};
use auction_domain::{CommandId, EquipmentId, EquipmentKeyEnvelope};
use auction_time::Timestamp;
use serde_json::json;
use uuid::Uuid;

use super::fixtures::{Listing, auction, commit, seed};
use super::production::lots::unassociated_collision_lots;
use crate::support::TestDatabase;

fn fake_envelope() -> EquipmentKeyEnvelope {
    EquipmentKeyEnvelope::new(
        vec![1; 32],
        vec![2; 12],
        "fake-encryption-key".into(),
        "a".repeat(64),
    )
    .expect("合成测试信封")
}

#[tokio::test]
async fn auction_mail_and_admin_writes_create_immediate_collision() {
    let db = TestDatabase::create().await.expect("临时 PG18");
    let owner = db.maintenance_store().await.expect("维护连接");
    let runtime = db.store().await.expect("服务连接");
    let pool = owner.postgres_pool();
    let service = runtime.postgres_pool();
    seed(pool).await;
    let mut tx = pool.begin().await.expect("拍卖录入目标");
    auction(&mut tx, "main", 1, "draft").await;
    commit(tx).await;
    let mut tx = service.begin().await.expect("录入前登记");
    let mail_listings = [Listing::new(101, "42"), Listing::new(102, "43")];
    let admin_listing = Listing::new(103, "44");
    for listing in [&mail_listings[0], &mail_listings[1], &admin_listing] {
        listing.register(&mut tx).await;
    }
    commit(tx).await;

    let now: Timestamp = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(service)
        .await
        .expect("录入数据库时刻");
    let mail = MailAuctionIntakeWrite {
        command_id: CommandId::from_uuid(Uuid::new_v4()),
        aggregate_id: Uuid::new_v4(),
        world_id: "main".into(),
        mailbox_key: "mooglemail:main".into(),
        external_mail_id: "7000002".into(),
        seller: "FakeSeller".into(),
        received_at: now,
        now,
        submission_id: Uuid::new_v4(),
        request_hash: RequestHash::new([4; 32]),
        items: ["42", "43"]
            .into_iter()
            .map(|eid| MailAuctionItemWrite {
                equipment_id: EquipmentId::from_uuid(Uuid::new_v4()),
                lot_id: Uuid::new_v4(),
                external_equipment_id: eid.into(),
                name: "Fake Weapon".into(),
                category: "Weapon".into(),
                level: Some(100),
                attributes: json!({}),
                collected_at: now,
                start_price: 50000,
                key_envelope: fake_envelope(),
            })
            .collect(),
    };
    let mail = runtime
        .run(TransactionPolicy::serializable(), |tx| {
            let write = mail.clone();
            Box::pin(async move {
                let record = MailAuctionIntakeRecord {
                    command_id: write.command_id,
                    world_id: write.world_id.clone(),
                    mailbox_key: write.mailbox_key.clone(),
                    result: tx.add_mail_auction_intake(write).await?,
                };
                let result = record.result.clone();
                tx.append_mail_auction_record(record).await?;
                Ok::<_, ApplicationError>(result)
            })
        })
        .await
        .expect("已提交多附件邮件 lot");

    let admin = AdminIntakeWrite {
        command_id: CommandId::from_uuid(Uuid::new_v4()),
        aggregate_id: Uuid::new_v4(),
        world_id: "main".into(),
        source_kind: AdminIntakeSourceKind::Json,
        source_reference: "manual-json-v1:00044".into(),
        source_fingerprint: [5; 32],
        actor_id: "fake-admin".into(),
        request_id: None,
        player_id: "FakeSeller".into(),
        start_price: 50000,
        candidate_equipment_id: EquipmentId::from_uuid(Uuid::new_v4()),
        candidate_submission_id: Uuid::new_v4(),
        candidate_lot_id: Uuid::new_v4(),
        external_equipment_id: "00044".into(),
        equipment_name: "Fake Weapon".into(),
        equipment_category: "Weapon".into(),
        equipment_level: Some(100),
        equipment_attributes: json!({}),
        collected_at: now,
        key_envelope: fake_envelope(),
        request_hash: RequestHash::new([6; 32]),
    };
    let admin = runtime
        .run(TransactionPolicy::serializable(), |tx| {
            let write = admin.clone();
            Box::pin(async move {
                let record = AdminIntakeRecord {
                    command_id: write.command_id,
                    world_id: write.world_id.clone(),
                    actor_id: write.actor_id.clone(),
                    request_id: write.request_id.clone(),
                    result: tx.add_admin_intake(write).await?,
                };
                let result = record.result.clone();
                tx.append_admin_intake_record(record).await?;
                Ok::<_, ApplicationError>(result)
            })
        })
        .await
        .expect("已提交 Admin lot");

    let mut read = service.acquire().await.expect("新读快照");
    assert_eq!(mail.lot_ids.len(), 2);
    for (listing, lot_id) in
        mail_listings.iter().zip(mail.lot_ids).chain([(&admin_listing, admin.lot_id)])
    {
        assert_eq!(
            unassociated_collision_lots(&mut read, listing.listing_id).await.expect("写入即碰撞"),
            [lot_id]
        );
    }
}
