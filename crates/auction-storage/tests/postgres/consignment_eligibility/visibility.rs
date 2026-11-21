//! 公开可见性、清退优先序、占用、时间窗口及账户匹配。

use auction_domain::UserId;
use auction_domain::consignment::ListingReason;
use auction_time::{Duration, Timestamp};
use sqlx::PgConnection;
use uuid::Uuid;

use super::fixtures::{
    END, LATER, Listing, OTHER, SELLER, START, at, auction, batch, commit, equip, gear, lot, seed,
};
use super::production::accounts::{OwnerMatch, lock_seller_eligibility, match_current_owner};
use super::production::occupancy::{
    effective_activity_count, hourly_registration_count, lock_equipment_occupants,
};
use super::production::predicates::listing_sql;
use crate::support::{TestDatabase, fixtures};

async fn view(
    cx: &mut PgConnection,
    listing_id: Uuid,
    as_of: Timestamp,
) -> (bool, bool, Option<String>) {
    sqlx::query_as(concat!(
        "SELECT ",
        listing_sql!(public_visible, "$2"),
        ", ",
        listing_sql!(effective_active, "$2"),
        ", ",
        listing_sql!(forced_reason, "$2"),
        " FROM public.consignment_listings l",
        listing_sql!(eligibility_joins),
        " WHERE l.id = $1"
    ))
    .bind(listing_id)
    .bind(as_of)
    .fetch_one(cx)
    .await
    .expect("查询同快照的三项谓词")
}

fn rejected(reason: ListingReason) -> (bool, bool, Option<String>) {
    (false, false, Some(reason.as_str().into()))
}

#[tokio::test]
async fn visibility_uses_prioritized_forced_reasons_and_snapshot() {
    let db = TestDatabase::create().await.expect("临时 PG18");
    let owner = db.maintenance_store().await.expect("维护连接");
    let runtime = db.store().await.expect("服务连接");
    let pool = owner.postgres_pool();
    let service = runtime.postgres_pool();
    seed(pool).await;
    let displayed = Listing::new(101, "42");
    let awaiting = Listing { mode: "consigned", ..Listing::new(102, "43") };
    let resolving = Listing::new(103, "44");
    let old_epoch = Listing { epoch: 2, ..Listing::new(104, "45") };
    let mut tx = service.begin().await.expect("活动挂单");
    for listing in [&displayed, &awaiting, &resolving, &old_epoch] {
        listing.register(&mut tx).await;
    }
    for listing in [&displayed, &awaiting, &old_epoch] {
        listing.publish(&mut tx).await;
    }
    commit(tx).await;

    let mut read = service.acquire().await.expect("可见性读取");
    let now = at(END) - Duration::microseconds(1);
    for listing in [&displayed, &awaiting] {
        assert_eq!(view(&mut read, listing.listing_id, now).await, (true, true, None));
    }
    assert_eq!(view(&mut read, resolving.listing_id, now).await, (false, true, None));
    for (listing, reason) in [
        (&displayed, ListingReason::DisplayExpired),
        (&awaiting, ListingReason::DepositExpired),
        (&resolving, ListingReason::LookupDeadline),
    ] {
        assert_eq!(view(&mut read, listing.listing_id, at(END)).await, rejected(reason));
    }
    assert_eq!(
        view(&mut read, old_epoch.listing_id, now).await,
        rejected(ListingReason::ServiceDisabled)
    );

    let mut admin = pool.acquire().await.expect("模拟停用及拉黑写入");
    batch(&mut admin, "UPDATE public.consignment_lookup_control SET admitting=false;").await;
    assert_eq!(
        view(&mut read, displayed.listing_id, at(END)).await,
        rejected(ListingReason::ServiceDisabled)
    );
    batch(&mut admin, "UPDATE public.consignment_lookup_control SET admitting=true;").await;
    batch(&mut admin, &format!("UPDATE public.users SET status='blacklisted' WHERE id='{SELLER}'"))
        .await;
    assert_eq!(
        view(&mut read, displayed.listing_id, now).await,
        rejected(ListingReason::SellerIneligible)
    );
    batch(&mut admin, &format!("UPDATE public.users SET status='active' WHERE id='{SELLER}'"))
        .await;
    batch(
        &mut admin,
        &format!(
            "INSERT INTO public.user_blacklist(user_id,public_reason,actor,created_at)
        VALUES ('{SELLER}','Fake reason','fake-actor','{START}')"
        ),
    )
    .await;
    assert_eq!(
        view(&mut read, displayed.listing_id, now).await,
        rejected(ListingReason::SellerIneligible)
    );
    batch(
        &mut admin,
        &format!(
            "UPDATE public.user_blacklist SET revoked_at='{LATER}' WHERE user_id='{SELLER}';
         INSERT INTO public.user_blacklist_history(id,user_id,public_reason,actor,blacklisted_at)
         VALUES (gen_random_uuid(),'{SELLER}','Fake reason','fake-actor','{START}');"
        ),
    )
    .await;
    assert_eq!(
        view(&mut read, displayed.listing_id, now).await,
        rejected(ListingReason::SellerIneligible)
    );
    batch(&mut admin, "UPDATE public.consignment_lookup_control SET admitting=false;").await;
    assert_eq!(
        view(&mut read, displayed.listing_id, at(END)).await,
        rejected(ListingReason::SellerIneligible)
    );

    let mut writer = pool.begin().await.expect("提交碰撞");
    let equipment_id = equip(&mut writer, "main", "00042").await;
    let auction_id = auction(&mut writer, "main", 1, "draft").await;
    lot(&mut writer, Uuid::new_v4(), auction_id, equipment_id, 1, "pending", None).await;
    commit(writer).await;
    assert_eq!(
        view(&mut read, displayed.listing_id, at(END)).await,
        rejected(ListingReason::AuctionLotCreated)
    );

    let after_unblacklist = Listing { blacklist_count: 1, ..Listing::new(105, "46") };
    batch(&mut admin, "UPDATE public.consignment_lookup_control SET admitting=true;").await;
    let mut tx = service.begin().await.expect("解禁后新挂单");
    after_unblacklist.register(&mut tx).await;
    after_unblacklist.publish(&mut tx).await;
    assert_eq!(view(&mut tx, after_unblacklist.listing_id, now).await, (true, true, None));
    tx.rollback().await.expect("结束夹具");
}

#[tokio::test]
async fn quotas_use_fixed_time_and_count_both_worlds() {
    let db = TestDatabase::create().await.expect("临时 PG18");
    let owner = db.maintenance_store().await.expect("维护连接");
    let runtime = db.store().await.expect("服务连接");
    seed(owner.postgres_pool()).await;
    let mut tx = runtime.postgres_pool().begin().await.expect("额度事务");
    let seller_id = UserId::from_uuid(SELLER);
    let as_of = at(END);
    for entry in [
        Listing { created: "2026-01-01T23:00:00Z", ..Listing::new(101, "101") },
        Listing { created: "2026-01-01T23:00:00.000001Z", ..Listing::new(102, "102") },
        Listing { created: END, world: "isekai", ..Listing::new(103, "103") },
        Listing {
            created: "2026-01-02T00:00:00.000001Z",
            seller: OTHER,
            ..Listing::new(104, "104")
        },
        Listing { created: "2026-01-02T00:00:00.000001Z", epoch: 2, ..Listing::new(105, "105") },
        Listing::new(106, "106"),
        Listing { epoch: 2, created: LATER, ..Listing::new(107, "107") },
        Listing { blacklist_count: 1, created: LATER, ..Listing::new(108, "108") },
    ] {
        entry.register(&mut tx).await;
    }
    assert_eq!(hourly_registration_count(&mut tx, seller_id, as_of).await.expect("小时计数"), 2);
    assert_eq!(effective_activity_count(&mut tx, seller_id, as_of).await.expect("跨两世界计数"), 3);
    assert_eq!(
        hourly_registration_count(&mut tx, UserId::from_uuid(OTHER), as_of)
            .await
            .expect("其他卖家"),
        0
    );
    assert_eq!(
        effective_activity_count(&mut tx, UserId::from_uuid(Uuid::nil()), as_of)
            .await
            .expect("缺失卖家"),
        0
    );
    commit(tx).await;
}

#[tokio::test]
async fn same_equipment_occupancy_distinguishes_cleanup_and_cross_mode_duplicates() {
    let db = TestDatabase::create().await.expect("临时 PG18");
    let owner = db.maintenance_store().await.expect("维护连接");
    let runtime = db.store().await.expect("服务连接");
    seed(owner.postgres_pool()).await;
    let mut tx = runtime.postgres_pool().begin().await.expect("占用事务");
    let active = Listing::new(101, "42");
    let expired = Listing {
        seller: OTHER,
        mode: "consigned",
        expires: "2026-01-01T06:00:00Z",
        ..Listing::new(102, "42")
    };
    let other_eid = Listing { seller: OTHER, epoch: 2, ..Listing::new(103, "43") };
    let other_world = Listing { world: "isekai", ..Listing::new(104, "42") };
    let published = Listing { seller: OTHER, mode: "consigned", ..Listing::new(105, "44") };
    for listing in [&active, &expired, &other_eid, &other_world, &published] {
        listing.register(&mut tx).await;
    }
    for listing in [&expired, &published] {
        listing.publish(&mut tx).await;
    }
    let as_of = at("2026-01-01T06:00:00Z");
    let rows = lock_equipment_occupants(&mut tx, &gear("main", "42"), as_of).await.expect("同件锁");
    assert_eq!(rows.len(), 2);
    assert_eq!(
        (rows[0].listing_id, rows[0].seller_user_id, rows[0].published, rows[0].forced_reason),
        (active.listing_id, UserId::from_uuid(SELLER), false, None)
    );
    assert_eq!(
        (rows[1].listing_id, rows[1].seller_user_id, rows[1].published, rows[1].forced_reason),
        (expired.listing_id, UserId::from_uuid(OTHER), true, Some(ListingReason::DepositExpired))
    );
    let rows =
        lock_equipment_occupants(&mut tx, &gear("main", "44"), as_of).await.expect("核验占用");
    assert_eq!(rows.len(), 1);
    assert!(rows[0].published);
    assert_eq!(rows[0].forced_reason, None);
    assert!(
        lock_equipment_occupants(&mut tx, &gear("main", "47"), as_of)
            .await
            .expect("无活动行")
            .is_empty()
    );
    commit(tx).await;
}

#[tokio::test]
async fn owner_cardinality_and_seller_history_are_independent() {
    let db = TestDatabase::create().await.expect("临时 PG18");
    let owner = db.maintenance_store().await.expect("维护连接");
    let runtime = db.store().await.expect("服务连接");
    let pool = owner.postgres_pool();
    let service = runtime.postgres_pool();
    seed(pool).await;
    let seller_id = UserId::from_uuid(SELLER);
    let mut tx = service.begin().await.expect("事务连接");
    for (name, expected) in [
        ("fAkEsElLeR", OwnerMatch::Seller),
        ("FakeOther", OwnerMatch::Other),
        ("OldName", OwnerMatch::Missing),
    ] {
        assert_eq!(
            match_current_owner(&mut tx, seller_id, name).await.expect("当前 owner"),
            expected
        );
    }
    let facts =
        lock_seller_eligibility(&mut tx, seller_id).await.expect("资格锁").expect("现有用户");
    assert!(facts.active);
    assert!(!facts.active_blacklist);
    assert_eq!(facts.blacklist_count, 0);
    assert!(
        lock_seller_eligibility(&mut tx, UserId::from_uuid(Uuid::nil()))
            .await
            .expect("缺失用户")
            .is_none()
    );
    commit(tx).await;

    let mut admin = pool.acquire().await.expect("隔离库 DDL");
    batch(
        &mut admin,
        &format!(
            "DROP INDEX public.users_username_case_insensitive_unique;
         UPDATE public.users SET status='blacklisted' WHERE id='{SELLER}';
         INSERT INTO public.user_blacklist(user_id,public_reason,actor,created_at)
             VALUES ('{SELLER}','Fake reason','fake-actor','{START}');
         INSERT INTO public.user_blacklist_history(id,user_id,public_reason,actor,blacklisted_at)
             VALUES (gen_random_uuid(),'{SELLER}','Fake reason','fake-actor','{START}');"
        ),
    )
    .await;
    fixtures::seed_user(pool, Uuid::new_v4(), "FAKESELLER").await;
    let mut tx = service.begin().await.expect("重名及资格事务");
    assert_eq!(
        match_current_owner(&mut tx, seller_id, "FakeSeller").await.expect("重名歧义"),
        OwnerMatch::Ambiguous
    );
    let facts =
        lock_seller_eligibility(&mut tx, seller_id).await.expect("资格锁").expect("用户存在");
    assert!(!facts.active);
    assert!(facts.active_blacklist);
    assert_eq!(facts.blacklist_count, 1);
    commit(tx).await;
}

#[tokio::test]
async fn public_visibility_respects_commit_snapshots_and_snapshot_identity() {
    let db = TestDatabase::create().await.expect("临时 PG18");
    let owner = db.maintenance_store().await.expect("维护连接");
    let runtime = db.store().await.expect("服务连接");
    let pool = owner.postgres_pool();
    let service = runtime.postgres_pool();
    seed(pool).await;

    let displayed = Listing::new(101, "42");
    let unrelated = Listing::new(102, "43");
    let mut tx = service.begin().await.expect("发布活动挂单");
    for listing in [&displayed, &unrelated] {
        listing.register(&mut tx).await;
        listing.publish(&mut tx).await;
    }
    commit(tx).await;

    let mut conflicted = pool.begin().await.expect("未提交的跨单快照");
    batch(&mut conflicted, &format!(
        "ALTER TABLE public.consignment_listings DISABLE TRIGGER consignment_guard_listing;
         UPDATE public.consignment_listings SET current_snapshot_id=(SELECT current_snapshot_id FROM public.consignment_listings WHERE id='{}')
         WHERE id='{}';", unrelated.listing_id, displayed.listing_id
    )).await;
    assert_eq!(view(&mut conflicted, displayed.listing_id, at(LATER)).await, (false, true, None));
    conflicted.rollback().await.expect("回滚跨单快照");

    let mut before = service
        .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .await
        .expect("提交前读快照");
    assert_eq!(view(&mut before, displayed.listing_id, at(LATER)).await, (true, true, None));
    let mut writer = pool.begin().await.expect("准备别名 lot");
    let equipment_id = equip(&mut writer, "main", "00042").await;
    let auction_id = auction(&mut writer, "main", 1, "draft").await;
    lot(&mut writer, Uuid::new_v4(), auction_id, equipment_id, 1, "pending", None).await;
    commit(writer).await;
    assert_eq!(view(&mut before, displayed.listing_id, at(LATER)).await, (true, true, None));
    let mut after = service.acquire().await.expect("提交后读快照");
    assert_eq!(
        view(&mut after, displayed.listing_id, at(LATER)).await,
        rejected(ListingReason::AuctionLotCreated)
    );
    assert_eq!(view(&mut after, unrelated.listing_id, at(LATER)).await, (true, true, None));
    commit(before).await;
}
