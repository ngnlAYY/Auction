//! 基线冻结、快照可见性与批量或单点碰撞关联。

use auction_domain::UserId;
use sqlx::Connection;
use uuid::Uuid;

use super::fixtures::{
    LATER, Listing, SELLER, START, at, auction, batch, commit, equip, gear, lot, seed, submission,
};
use super::production::history::read_listing_history;
use super::production::lots::{
    INSERT_COLLISION_SET_SQL, INSERT_OBSERVED_COLLISION_SQL, insert_registration_baselines,
    listing_collision, observed_collision_is_unassociated, unassociated_collision_lots,
};
use crate::support::TestDatabase;

#[tokio::test]
async fn postgres_alias_baseline_and_snapshot() {
    let db = TestDatabase::create().await.expect("临时 PG18");
    let owner = db.maintenance_store().await.expect("维护连接");
    let runtime = db.store().await.expect("服务连接");
    let pool = owner.postgres_pool();
    let service = runtime.postgres_pool();
    seed(pool).await;
    let mut setup = pool.begin().await.expect("初始拍卖事务");
    let canonical = equip(&mut setup, "main", "42").await;
    let alias = equip(&mut setup, "main", "00042").await;
    let long_alias = equip(&mut setup, "main", &format!("{}42", "0".repeat(62))).await;
    let zero = equip(&mut setup, "main", "000").await;
    let non_numeric = equip(&mut setup, "main", "42x").await;
    let oversized = equip(&mut setup, "main", &"4".repeat(64)).await;
    let other_world = equip(&mut setup, "isekai", "42").await;
    let old_auction = auction(&mut setup, "main", 1, "settled").await;
    let future_auction = auction(&mut setup, "main", 2, "draft").await;
    let isekai_auction = auction(&mut setup, "isekai", 1, "settled").await;
    let old_ids = [Uuid::from_u128(999), Uuid::from_u128(998), Uuid::from_u128(997)];
    for (index, equipment) in
        [canonical, alias, long_alias, zero, non_numeric, oversized].into_iter().enumerate()
    {
        let number = i32::try_from(index + 1).expect("lot 号");
        let lot_id = old_ids.get(index).copied().unwrap_or_else(Uuid::new_v4);
        lot(&mut setup, lot_id, old_auction, equipment, number, "unsold", None).await;
    }
    lot(&mut setup, Uuid::new_v4(), isekai_auction, other_world, 1, "unsold", None).await;
    commit(setup).await;

    let direct = Listing::new(101, "42");
    let consigned =
        Listing { mode: "consigned", seller: super::fixtures::OTHER, ..Listing::new(102, "42") };
    let mut writer = pool.begin().await.expect("并发拍卖连接");
    let later_lot = Uuid::from_u128(7);
    lot(&mut writer, later_lot, future_auction, alias, 1, "pending", None).await;

    let mut db_conn = service.acquire().await.expect("登记连接");
    let mut registering =
        db_conn.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await.expect("串行登记");
    let history =
        read_listing_history(&mut registering, &gear("main", "42")).await.expect("历史读取");
    assert_eq!(history.aliases.len(), 3);
    let mut lot_ids = history.lot_ids;
    lot_ids.sort_unstable();
    assert_eq!(lot_ids, [old_ids[2], old_ids[1], old_ids[0]]);
    commit(writer).await;
    for new_listing in [direct, consigned] {
        new_listing.register(&mut registering).await;
        assert_eq!(
            insert_registration_baselines(&mut registering, new_listing.listing_id)
                .await
                .expect("冻结基线"),
            3
        );
        assert_eq!(
            listing_collision(&mut registering, new_listing.listing_id).await.expect("旧快照碰撞"),
            Some(false)
        );
    }
    commit(registering).await;

    let mut reading = service.acquire().await.expect("提交后新快照");
    for listing_id in [Uuid::from_u128(101), Uuid::from_u128(102)] {
        let baseline = sqlx::query_scalar::<_, Uuid>(
            "SELECT lot_id FROM public.consignment_listing_lot_baselines WHERE listing_id=$1 ORDER BY lot_id"
        ).bind(listing_id).fetch_all(&mut *reading).await.expect("登记集合");
        assert_eq!(baseline, [old_ids[2], old_ids[1], old_ids[0]]);
        assert_eq!(
            listing_collision(&mut reading, listing_id).await.expect("提交后碰撞"),
            Some(true)
        );
        assert_eq!(
            unassociated_collision_lots(&mut reading, listing_id).await.expect("碰撞差集"),
            [later_lot]
        );
    }
}

#[tokio::test]
async fn postgres_collision_tracks_committed_lots_and_associates_only_observed_facts() {
    let db = TestDatabase::create().await.expect("临时 PG18");
    let owner = db.maintenance_store().await.expect("维护连接");
    let runtime = db.store().await.expect("服务连接");
    let pool = owner.postgres_pool();
    let service = runtime.postgres_pool();
    seed(pool).await;

    let listing = Listing::new(101, "42");
    let listing_id = listing.listing_id;
    let mut tx = service.begin().await.expect("登记事务");
    listing.register(&mut tx).await;
    assert_eq!(insert_registration_baselines(&mut tx, listing_id).await.expect("空基线"), 0);
    commit(tx).await;

    let mut writer = pool.begin().await.expect("拍卖事务");
    let mail_auction = auction(&mut writer, "main", 1, "settled").await;
    let admin_auction = auction(&mut writer, "main", 2, "settled").await;
    let relist_auction = auction(&mut writer, "main", 3, "draft").await;
    let mail_equipment = equip(&mut writer, "main", "42").await;
    let admin_equipment = equip(&mut writer, "main", "00042").await;
    let mail_submission = submission(&mut writer, mail_auction, "mail", "FakeSeller", START).await;
    let admin_submission =
        submission(&mut writer, admin_auction, "admin", "FakeSeller", START).await;
    batch(&mut writer, &format!("INSERT INTO public.inbound_mail
        (id,mailbox_key,external_mail_id,world_id,mail_kind,content_digest,received_at,status,decision_code,processed_at)
        VALUES (gen_random_uuid(),'fake-mailbox','999','main','equipment',
                decode(repeat('11',32),'hex'),'{START}','rejected','fake_rejected','{START}');")).await;
    commit(writer).await;

    let mut writer = pool.begin().await.expect("回滚拍卖事务");
    lot(
        &mut writer,
        Uuid::from_u128(90),
        mail_auction,
        mail_equipment,
        1,
        "pending",
        Some(mail_submission),
    )
    .await;
    let mut read = service.acquire().await.expect("读取连接");
    assert_eq!(listing_collision(&mut read, listing_id).await.expect("未提交不见"), Some(false));
    writer.rollback().await.expect("建 lot 回滚");
    assert_eq!(
        listing_collision(&mut read, listing_id).await.expect("拒信与回滚不见"),
        Some(false)
    );

    let mail_lot = Uuid::from_u128(91);
    let admin_lot = Uuid::from_u128(92);
    let relist_lot = Uuid::from_u128(93);
    let mut writer = pool.begin().await.expect("提交拍卖事务");
    lot(&mut writer, mail_lot, mail_auction, mail_equipment, 1, "unsold", Some(mail_submission))
        .await;
    lot(
        &mut writer,
        admin_lot,
        admin_auction,
        admin_equipment,
        1,
        "withdrawn",
        Some(admin_submission),
    )
    .await;
    commit(writer).await;

    let mut closing = service.begin().await.expect("碰撞关闭事务");
    assert_eq!(
        unassociated_collision_lots(&mut closing, listing_id).await.expect("两源差集"),
        [mail_lot, admin_lot]
    );
    assert!(
        !observed_collision_is_unassociated(&mut closing, listing_id, relist_lot)
            .await
            .expect("未见的 lot")
    );
    let close_command = Uuid::new_v5(&listing_id, b"fixture-auto-close");
    batch(&mut closing, &format!(r#"
        UPDATE public.consignment_listings SET state='closed',state_version=2,
               reason_code='auction_lot_created',updated_at='{LATER}' WHERE id='{listing_id}';
        INSERT INTO public.consignment_commands
            (command_id,actor_kind,actor_ref,operation,request_target,request_digest,listing_id,
             result_status,result_code,result_body,created_at)
        VALUES ('{close_command}','system','fake-system','auto_close','{listing_id}',
                decode(repeat('22',32),'hex'),'{listing_id}',200,'ok',
                '{{"command_id":"{close_command}","operation":"auto_close","listing_id":"{listing_id}",
                   "listing_version":2,"state":"closed","disposition":"closed","price":101,
                   "expires_at":"2026-01-02T00:00:00Z","reason_code":"auction_lot_created"}}','{LATER}');
        INSERT INTO public.consignment_events
            (listing_id,state_version,command_id,actor_kind,event_kind,from_state,to_state,reason_code,price,occurred_at)
        VALUES ('{listing_id}',2,'{close_command}','system','closed','resolving','closed','auction_lot_created',101,'{LATER}');
    "#)).await;
    assert_eq!(
        sqlx::query(INSERT_COLLISION_SET_SQL)
            .bind(listing_id)
            .bind(at(LATER))
            .bind(close_command)
            .execute(&mut *closing)
            .await
            .expect("集合追加")
            .rows_affected(),
        2
    );
    assert!(
        unassociated_collision_lots(&mut closing, listing_id).await.expect("已经关联").is_empty()
    );
    assert_eq!(
        listing_collision(&mut closing, listing_id).await.expect("关联后仍碰撞"),
        Some(true)
    );
    commit(closing).await;
    drop(read);

    let mut writer = pool.begin().await.expect("恢复重列事务");
    lot(
        &mut writer,
        relist_lot,
        relist_auction,
        mail_equipment,
        1,
        "pending",
        Some(mail_submission),
    )
    .await;
    commit(writer).await;
    let mut observed = service.begin().await.expect("指定事实追加事务");
    assert!(
        observed_collision_is_unassociated(&mut observed, listing_id, relist_lot)
            .await
            .expect("重列复用投稿仍碰撞")
    );
    assert!(
        !observed_collision_is_unassociated(&mut observed, listing_id, mail_lot)
            .await
            .expect("重复观察")
    );
    for (candidate, expected) in [(Uuid::nil(), 0), (mail_lot, 0), (relist_lot, 1), (relist_lot, 0)]
    {
        assert_eq!(
            sqlx::query(INSERT_OBSERVED_COLLISION_SQL)
                .bind(listing_id)
                .bind(at(LATER))
                .bind(close_command)
                .bind(candidate)
                .execute(&mut *observed)
                .await
                .expect("单点追加")
                .rows_affected(),
            expected
        );
    }
    commit(observed).await;
    let stored: (String, i64) = sqlx::query_as(
        "SELECT reason_code,state_version FROM public.consignment_listings WHERE id=$1",
    )
    .bind(listing_id)
    .fetch_one(service)
    .await
    .expect("首次终态");
    assert_eq!(stored, ("auction_lot_created".into(), 2));
    let current: Uuid =
        sqlx::query_scalar("SELECT seller_user_id FROM public.consignment_listings WHERE id=$1")
            .bind(listing_id)
            .fetch_one(service)
            .await
            .expect("卖家未改变");
    assert_eq!(UserId::from_uuid(current), UserId::from_uuid(SELLER));
}
