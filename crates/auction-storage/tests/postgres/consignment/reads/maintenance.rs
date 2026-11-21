//! 扫描位置持久化、回绕与强制事实的证据优先序。

use auction_application::consignment::{
    AutoCloseCandidate, AutoCloseEvidence, AutoCloseListingCommand, ListingAutoCloseEffect,
    ListingCommandStore, ListingMaintenance,
};
use auction_application::core::RequestHash;
use auction_domain::consignment::ListingReason;
use auction_domain::{CommandId, LotId};
use auction_time::Duration;
use uuid::Uuid;

use super::fixture::{Fixture, HISTORY, Listing, SELLER};

#[tokio::test]
async fn postgres_worker_maintenance_port_finds_a_committed_collision() {
    let fixture = Fixture::new().await;
    let listing = Listing::new(&fixture, 601, "42");
    let baseline = fixture.lot("main", "00042", 1).await;
    listing.seed(&fixture, true, Some(baseline)).await;
    fixture.lot("isekai", "42", 3).await;

    let maintenance: &dyn ListingMaintenance = &fixture.store;
    let page = maintenance.scan_candidates().await.unwrap();
    assert_eq!(page.scanned, 1);
    assert!(page.candidates.is_empty());

    let collision = fixture.lot("main", "00000042", 2).await;
    let page = maintenance.scan_candidates().await.unwrap();
    assert_eq!(page.scanned, 1);
    assert_eq!(page.candidates.len(), 1);
    assert_eq!(page.candidates[0].listing_id, listing.id);
    assert_eq!(
        page.candidates[0].evidence,
        AutoCloseEvidence::AuctionLotCreated(LotId::from_uuid(collision))
    );
}

#[tokio::test]
async fn postgres_worker_stock_probe_includes_unswept_listings_after_disable() {
    let fixture = Fixture::create(false).await;
    assert!(!fixture.store.has_active_consignment_listings().await.unwrap());

    let first = fixture.store.sync_admission(true).await.unwrap();
    Listing::new(&fixture, 601, "42").seed(&fixture, false, None).await;
    let disabled = fixture.store.sync_admission(false).await.unwrap();
    assert!(!disabled.admitting);
    assert_eq!(disabled.service_epoch, first.service_epoch + 1);
    assert!(fixture.store.has_active_consignment_listings().await.unwrap());

    let repeated = fixture.store.sync_admission(false).await.unwrap();
    assert_eq!(repeated, disabled);
}

#[tokio::test]
async fn postgres_scan_idle_leaves_control_untouched() {
    let fixture = Fixture::create(false).await;
    let read_control = || {
        sqlx::query_as::<_, (String, Option<Uuid>)>(
            "SELECT xmin::text, scan_after FROM public.consignment_lookup_control",
        )
        .fetch_one(fixture.pool())
    };
    let before = read_control().await.unwrap();
    // 常规补扫会读取 lot 以判断碰撞，空库的活动挂单存在性检查无需读取 lot。
    let mut blocker = fixture.pool().begin().await.unwrap();
    sqlx::query("LOCK TABLE public.lots IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let page = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        fixture.store.scan_consignment_candidates(),
    )
    .await
    .expect("空库补扫应在 lot 表仍被锁住时返回")
    .expect("空库补扫无需读取已锁住的 lot 表");
    blocker.rollback().await.unwrap();
    assert_eq!((page.scanned, page.scan_after), (0, None));
    assert!(page.candidates.is_empty());
    assert_eq!(read_control().await.unwrap(), before);
}

#[tokio::test]
async fn postgres_scan_keyset_advances_over_non_candidates_and_wraps() {
    let fixture = Fixture::new().await;
    for number in 100..=152 {
        let eid = (number + 10_000).to_string();
        let mut listing = Listing::new(&fixture, number, &eid);
        if number == 152 {
            listing.created = fixture.now - Duration::hours(25);
        }
        listing.seed(&fixture, false, None).await;
    }
    let first = fixture.store.scan_consignment_candidates().await.unwrap();
    let after = Uuid::from_u128(149);
    assert_eq!((first.scanned, first.scan_after), (50, Some(after)));
    assert!(first.candidates.is_empty());
    let persisted: Option<Uuid> =
        sqlx::query_scalar("SELECT scan_after FROM public.consignment_lookup_control")
            .fetch_one(fixture.pool())
            .await
            .unwrap();
    assert_eq!(persisted, Some(after));

    // 在游标之前插入的候选不能让正在扫描的旧尾页饥饿。
    let mut late = Listing::new(&fixture, 90, "9000");
    late.created = fixture.now - Duration::hours(25);
    late.seed(&fixture, false, None).await;
    let tail = fixture.store.scan_consignment_candidates().await.unwrap();
    assert_eq!((tail.scanned, tail.scan_after), (3, None));
    assert_eq!(
        tail.candidates.iter().map(|candidate| candidate.listing_id).collect::<Vec<_>>(),
        [Uuid::from_u128(152)]
    );
    let wrap = fixture.store.scan_consignment_candidates().await.unwrap();
    assert_eq!((wrap.scanned, wrap.scan_after), (50, Some(Uuid::from_u128(148))));
    assert_eq!(wrap.candidates.len(), 1);
    assert_eq!(wrap.candidates[0].listing_id, late.id);
    assert_eq!(
        wrap.candidates[0].evidence,
        AutoCloseEvidence::LookupDeadline(late.created + Duration::hours(24))
    );
}

#[tokio::test]
async fn postgres_scan_uses_priority_and_exact_persisted_evidence() {
    let fixture = Fixture::new().await;
    let baseline = fixture.lot("main", "00042", 1).await;
    let a = Listing { expires: fixture.now, ..Listing::new(&fixture, 501, "42") };
    let b =
        Listing { expires: fixture.now, mode: "consigned", ..Listing::new(&fixture, 502, "43") };
    let c = Listing::new(&fixture, 503, "44");
    a.seed(&fixture, true, Some(baseline)).await;
    b.seed(&fixture, true, None).await;
    c.seed(&fixture, false, None).await;
    let scan = async || {
        fixture
            .store
            .scan_consignment_candidates()
            .await
            .unwrap()
            .candidates
            .into_iter()
            .map(|candidate| (candidate.listing_id, candidate.evidence))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        scan().await,
        [
            (a.id, AutoCloseEvidence::DisplayExpired(a.expires)),
            (b.id, AutoCloseEvidence::DepositExpired(b.expires)),
        ]
    );
    fixture
        .sql("UPDATE public.consignment_lookup_control SET admitting = false, enabled_epoch = 2")
        .await;
    assert_eq!(
        scan().await,
        [
            (a.id, AutoCloseEvidence::ServiceDisabled(2)),
            (b.id, AutoCloseEvidence::ServiceDisabled(2)),
            (c.id, AutoCloseEvidence::ServiceDisabled(2)),
        ]
    );
    fixture.history(SELLER).await;
    assert_eq!(
        scan().await,
        [
            (a.id, AutoCloseEvidence::SellerIneligible(HISTORY)),
            (b.id, AutoCloseEvidence::SellerIneligible(HISTORY)),
            (c.id, AutoCloseEvidence::SellerIneligible(HISTORY)),
        ]
    );
    let lot_id = fixture.lot("main", "00042", 2).await;
    assert_eq!(
        scan().await,
        [
            (a.id, AutoCloseEvidence::AuctionLotCreated(LotId::from_uuid(lot_id))),
            (b.id, AutoCloseEvidence::SellerIneligible(HISTORY)),
            (c.id, AutoCloseEvidence::SellerIneligible(HISTORY)),
        ]
    );
    let resolving: String =
        sqlx::query_scalar("SELECT state FROM public.consignment_listings WHERE id = $1")
            .bind(c.id)
            .fetch_one(fixture.pool())
            .await
            .unwrap();
    assert_eq!(resolving, "resolving");
}

#[tokio::test]
async fn postgres_scan_missing_blacklist_evidence_cannot_fabricate_candidate() {
    let fixture = Fixture::new().await;
    fixture.history(SELLER).await;
    let a = Listing::new(&fixture, 501, "42");
    a.seed(&fixture, true, None).await;
    fixture
        .sql(&format!("UPDATE public.users SET status = 'blacklisted' WHERE id = '{SELLER}'"))
        .await;
    let page = fixture.store.scan_consignment_candidates().await.unwrap();
    assert_eq!(page.scanned, 1);
    assert!(page.candidates.is_empty());
    // 恢复动作可追加业务时刻早于登记的流水；基线内的事实仍不可作证。
    let recovered = Uuid::from_u128(401);
    fixture.history_at(SELLER, recovered, a.created - Duration::days(1)).await;
    let page = fixture.store.scan_consignment_candidates().await.unwrap();
    assert_eq!(page.candidates[0].evidence, AutoCloseEvidence::SellerIneligible(recovered));
}

#[tokio::test]
async fn postgres_scan_blacklist_same_time_lower_uuid_closes_with_new_evidence() {
    let fixture = Fixture::new().await;
    fixture.history(SELLER).await;
    let listing = Listing::new(&fixture, 501, "42");
    listing.seed(&fixture, true, None).await;
    let baseline: (i64, Vec<Uuid>) = sqlx::query_as(
        "SELECT blacklist_count_at_register, blacklist_baseline_ids
         FROM public.consignment_listings WHERE id = $1",
    )
    .bind(listing.id)
    .fetch_one(fixture.pool())
    .await
    .unwrap();
    assert_eq!(baseline, (1, vec![HISTORY]));
    let new_fact = Uuid::from_u128(399);
    fixture.history_at(SELLER, new_fact, fixture.now).await;
    let page = fixture.store.scan_consignment_candidates().await.unwrap();
    assert_eq!(
        page.candidates,
        [AutoCloseCandidate {
            listing_id: listing.id,
            evidence: AutoCloseEvidence::SellerIneligible(new_fact),
        }]
    );

    let command = AutoCloseListingCommand {
        request_hash: RequestHash::from_canonical_bytes(b"fake scan evidence verification"),
        command_id: CommandId::from_uuid(Uuid::new_v5(&listing.id, new_fact.as_bytes())),
        candidate: page.candidates[0],
    };
    let outcome = fixture.store.auto_close(command).await.unwrap().into_result().unwrap();
    assert!(matches!(
        outcome, ListingAutoCloseEffect::Terminated(snapshot)
            if snapshot.reason == Some(ListingReason::SellerIneligible)
    ));
    let closed: (String, i64, String) = sqlx::query_as(
        "SELECT state, state_version, reason_code FROM public.consignment_listings WHERE id = $1",
    )
    .bind(listing.id)
    .fetch_one(fixture.pool())
    .await
    .unwrap();
    assert_eq!(closed, ("closed".into(), 3, "seller_ineligible".into()));
}

#[tokio::test]
async fn postgres_scan_blacklist_orders_new_evidence_by_time_then_uuid() {
    let fixture = Fixture::new().await;
    let listing = Listing::new(&fixture, 501, "42");
    listing.seed(&fixture, false, None).await;

    let earlier_time = fixture.now - Duration::minutes(1);
    let earlier = Uuid::from_u128(403);
    fixture.history_at(SELLER, HISTORY, fixture.now).await;
    fixture.history_at(SELLER, earlier, earlier_time).await;
    let scan = async || fixture.store.scan_consignment_candidates().await.unwrap().candidates;
    let expected = |fact| {
        [AutoCloseCandidate {
            listing_id: listing.id,
            evidence: AutoCloseEvidence::SellerIneligible(fact),
        }]
    };
    assert_eq!(scan().await, expected(earlier));

    let tied_lower = Uuid::from_u128(402);
    fixture.history_at(SELLER, tied_lower, earlier_time).await;
    for _ in 0..2 {
        assert_eq!(scan().await, expected(tied_lower));
    }
}
