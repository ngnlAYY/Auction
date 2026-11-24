//! 公开条件、列表窗口、同快照总数及登记可用性。

use auction_application::consignment::{
    ListingCategoryFilter, ListingCredentialError, ListingEquipmentLevel, ListingNameQuery,
    ListingPageWindow, ListingRegistrationAvailability, ListingWorld, ListingWorlds,
    OwnedListingRead, PublicListingQuery, PublicListingRead, PublicListingState, RegistrationInput,
    RegistrationReplayCrypto, RegistrationRequestMac, WrappedRegistrationReplayKey,
};
use auction_domain::consignment::{ListingMode, ListingReason, ListingState};
use auction_time::Duration;
use uuid::Uuid;

use super::fixture::{Fixture, Listing, SELLER, public_query, seller, window};

async fn assert_hidden(
    fixture: &Fixture,
    listing: &Listing<'_>,
    expected: (ListingState, Option<ListingReason>),
    stored: ListingState,
) {
    let public = fixture.store.list_public(public_query()).await.expect("公开读");
    assert!(public.items.is_empty());
    assert_eq!(public.total, 0);
    assert!(fixture.store.public_detail(listing.id).await.expect("公开详情").is_none());
    let owned = fixture
        .store
        .owned_detail(seller(), listing.id)
        .await
        .expect("本人详情")
        .expect("本人可读");
    assert_eq!(owned.listing().view.stored_state(), stored);
    assert_eq!(owned.listing().view.effective_state(), expected);
    assert_eq!(owned.allowed_actions().is_empty(), !expected.0.is_active());
}

#[tokio::test]
async fn postgres_public_requires_publication_and_current_expiry() {
    let fixture = Fixture::new().await;
    let mut listing = Listing::new(&fixture, 500, "42");
    listing.seed(&fixture, false, None).await;
    assert_hidden(&fixture, &listing, (ListingState::Resolving, None), ListingState::Resolving)
        .await;
    // 两种冻结的到期原因与已发布模式对应，解析中也没有公开快照。
    for (id, eid, mode, stored, reason) in [
        (501, "43", "direct", ListingState::Displayed, ListingReason::DisplayExpired),
        (502, "44", "consigned", ListingState::AwaitingDeposit, ListingReason::DepositExpired),
    ] {
        let expired = Listing { mode, expires: fixture.now, ..Listing::new(&fixture, id, eid) };
        expired.seed(&fixture, true, None).await;
        assert_hidden(&fixture, &expired, (ListingState::Expired, Some(reason)), stored).await;
    }
    listing.id = Uuid::from_u128(503);
    listing.eid = "45";
    listing.created = fixture.now - Duration::hours(25);
    listing.seed(&fixture, false, None).await;
    assert_hidden(
        &fixture,
        &listing,
        (ListingState::Expired, Some(ListingReason::LookupDeadline)),
        ListingState::Resolving,
    )
    .await;
}

async fn eligible_listing(fixture: &Fixture, id: u128) -> Listing<'static> {
    let listing = Listing::new(fixture, id, "42");
    listing.seed(fixture, true, None).await;
    assert_eq!(fixture.store.list_public(public_query()).await.unwrap().total, 1);
    listing
}

async fn assert_forced_hidden(fixture: &Fixture, listing: &Listing<'_>, reason: ListingReason) {
    assert_hidden(fixture, listing, (ListingState::Closed, Some(reason)), ListingState::Displayed)
        .await;
}

#[tokio::test]
async fn postgres_public_admission_disabled_hides_listing() {
    let fixture = Fixture::new().await;
    let listing = eligible_listing(&fixture, 510).await;
    fixture.sql("UPDATE public.consignment_lookup_control SET admitting = false").await;
    assert_forced_hidden(&fixture, &listing, ListingReason::ServiceDisabled).await;
}

#[tokio::test]
async fn postgres_public_old_epoch_stays_hidden_after_reopening() {
    let fixture = Fixture::new().await;
    let listing = eligible_listing(&fixture, 511).await;
    fixture
        .sql("UPDATE public.consignment_lookup_control SET admitting = true, enabled_epoch = 2")
        .await;
    assert_forced_hidden(&fixture, &listing, ListingReason::ServiceDisabled).await;
}

#[tokio::test]
async fn postgres_public_inactive_seller_is_hidden() {
    let fixture = Fixture::new().await;
    let listing = eligible_listing(&fixture, 512).await;
    fixture
        .sql(&format!("UPDATE public.users SET status = 'blacklisted' WHERE id = '{SELLER}'"))
        .await;
    assert_forced_hidden(&fixture, &listing, ListingReason::SellerIneligible).await;
}

#[tokio::test]
async fn postgres_public_active_blacklist_is_hidden() {
    let fixture = Fixture::new().await;
    let listing = eligible_listing(&fixture, 513).await;
    fixture
        .sql(&format!(
            "INSERT INTO public.user_blacklist (user_id,public_reason,actor)
        VALUES ('{SELLER}','Fake reason','Fake operator')"
        ))
        .await;
    assert_forced_hidden(&fixture, &listing, ListingReason::SellerIneligible).await;
}

#[tokio::test]
async fn postgres_public_revoked_blacklist_history_stays_hidden() {
    let fixture = Fixture::new().await;
    let listing = eligible_listing(&fixture, 514).await;
    fixture
        .sql(&format!(
            "INSERT INTO public.user_blacklist (user_id,public_reason,actor,revoked_at)
        VALUES ('{SELLER}','Fake reason','Fake operator',transaction_timestamp())"
        ))
        .await;
    assert_eq!(fixture.store.list_public(public_query()).await.unwrap().total, 1);
    fixture.history(SELLER).await;
    assert_forced_hidden(&fixture, &listing, ListingReason::SellerIneligible).await;
}

#[tokio::test]
async fn postgres_public_committed_lot_hides_open_page_and_snapshot_is_consistent() {
    let fixture = Fixture::new().await;
    let baseline = fixture.lot("main", "00042", 1).await;
    let listing = Listing::new(&fixture, 500, "42");
    listing.seed(&fixture, true, Some(baseline)).await;
    fixture.lot("isekai", "00042", 2).await;
    let before = fixture.store.list_public(public_query()).await.unwrap();
    assert_eq!((before.total, before.items[0].listing_id), (1, listing.id));
    assert!(fixture.store.public_detail(listing.id).await.unwrap().is_some());

    let read = fixture
        .store
        .test_consignment_public_snapshot(public_query(), async |total| {
            assert_eq!(total, 1);
            fixture.lot("main", "00042", 3).await;
            // 独立请求已取得新的已提交事实，先前正在翻页的读仍使用旧快照。
            assert_hidden(
                &fixture,
                &listing,
                (ListingState::Closed, Some(ListingReason::AuctionLotCreated)),
                ListingState::Displayed,
            )
            .await;
        })
        .await
        .unwrap();
    assert_eq!(read, (1, 1));
}

async fn catalog_fixture(fixture: &Fixture) -> [Listing<'static>; 3] {
    let a = Listing { name: "Fake 100% Blade", ..Listing::new(fixture, 505, "42") };
    let b = Listing {
        mode: "consigned",
        world: "isekai",
        alias: Some("Fake_Plate"),
        category: "Armor",
        ..Listing::new(fixture, 503, "42")
    };
    let c = Listing::new(fixture, 501, "43");
    for listing in [&a, &b, &c] {
        listing.seed(fixture, true, None).await;
    }
    [a, b, c]
}

#[tokio::test]
async fn postgres_public_page_order_uses_id_for_tied_times() {
    let fixture = Fixture::new().await;
    let [a, b, c] = catalog_fixture(&fixture).await;
    for (page, expected) in [(1, a.id), (2, b.id), (3, c.id)] {
        let result = fixture
            .store
            .list_public(PublicListingQuery { window: window(page, 1), ..public_query() })
            .await
            .unwrap();
        assert_eq!((result.total, result.items[0].listing_id), (3, expected));
    }
}

#[tokio::test]
async fn postgres_public_filters_world_mode_category_and_literal_names() {
    let fixture = Fixture::new().await;
    let [a, b, c] = catalog_fixture(&fixture).await;
    let cases = [
        (
            PublicListingQuery { world: Some(ListingWorld::Main), ..public_query() },
            vec![a.id, c.id],
        ),
        (PublicListingQuery { mode: Some(ListingMode::Consigned), ..public_query() }, vec![b.id]),
        (
            PublicListingQuery {
                category: Some(ListingCategoryFilter::new("Armor".into()).unwrap()),
                ..public_query()
            },
            vec![b.id],
        ),
        (
            PublicListingQuery {
                category: Some(ListingCategoryFilter::new("armor".into()).unwrap()),
                ..public_query()
            },
            vec![],
        ),
        (
            PublicListingQuery {
                name: Some(ListingNameQuery::new("FAKE 100%".into()).unwrap()),
                ..public_query()
            },
            vec![a.id],
        ),
        (
            PublicListingQuery {
                name: Some(ListingNameQuery::new("_pLaTe".into()).unwrap()),
                ..public_query()
            },
            vec![b.id],
        ),
        (
            PublicListingQuery {
                name: Some(ListingNameQuery::new("_".into()).unwrap()),
                ..public_query()
            },
            vec![b.id],
        ),
        (
            PublicListingQuery {
                name: Some(ListingNameQuery::new("%".into()).unwrap()),
                ..public_query()
            },
            vec![a.id],
        ),
    ];
    for (query, expected) in cases {
        let page = fixture.store.list_public(query).await.unwrap();
        assert_eq!(page.total, expected.len() as u64);
        let mut ids: Vec<_> = page.items.iter().map(|item| item.listing_id).collect();
        ids.sort_unstable();
        let mut expected = expected;
        expected.sort_unstable();
        assert_eq!(ids, expected);
    }
}

#[tokio::test]
async fn postgres_public_projection_has_only_safe_fields() {
    let fixture = Fixture::new().await;
    let [a, b, _] = catalog_fixture(&fixture).await;
    fixture
        .sql(&format!("UPDATE public.users SET username = 'FakeRenamed' WHERE id = '{SELLER}'"))
        .await;
    let result = fixture
        .store
        .list_public(PublicListingQuery { window: window(1, 37), ..public_query() })
        .await
        .unwrap();
    assert_eq!((result.window.page_size(), result.total), (37, 3));
    let displayed = fixture.store.public_detail(a.id).await.unwrap().unwrap();
    let catalog_item = result.items.iter().find(|item| item.listing_id == a.id).unwrap();
    assert_eq!(displayed.summary.state, PublicListingState::Displayed);
    assert_eq!(displayed.summary.seller_name, "FakeRenamed");
    assert_eq!(&displayed.summary, catalog_item);
    assert_eq!(displayed.equipment.stat_groups()[0].stats[0].value, 4.5);
    let details = fixture.store.public_detail(b.id).await.unwrap().unwrap();
    let summary = &details.summary;
    assert_eq!(summary, &result.items[1]);
    assert_eq!(summary.listing_id, b.id);
    assert_eq!(summary.version.value(), 2);
    assert_eq!(summary.state, PublicListingState::AwaitingDeposit);
    assert_eq!(summary.identity.world_id(), "isekai");
    assert_eq!(summary.identity.external_equipment_id(), "42");
    assert_eq!(summary.name, "Fake Blade");
    assert_eq!(summary.category, "Armor");
    assert_eq!(summary.level, ListingEquipmentLevel::Assigned(100));
    assert_eq!(summary.price.value(), 101);
    assert_eq!(summary.seller_name, "FakeRenamed");
    assert_eq!(summary.updated_at, b.published);
    assert_eq!(summary.expires_at, b.expires);
    let equipment = &details.equipment;
    let label = equipment.label();
    assert_eq!(equipment.observed_at(), b.published);
    assert_eq!(label.name, "Fake Blade");
    assert_eq!(label.alt_name.as_deref(), Some("Fake_Plate"));
    assert_eq!(label.category, "Armor");
    assert_eq!(label.level, ListingEquipmentLevel::Assigned(100));
    assert!(label.tradeable);
    let [group] = equipment.stat_groups() else { panic!("快照应有一个属性组") };
    assert_eq!(group.name, "Basic");
    let [stat] = group.stats.as_slice() else { panic!("属性组应有一个属性") };
    assert_eq!(stat.name, "Power");
    assert_eq!(stat.value, 4.5);
    assert_eq!(stat.base, None);
    let text = format!("{:?} {displayed:?} {details:?}", result.items);
    for private in [
        SELLER.to_string().as_str(),
        "seller_user_id",
        "reason_code",
        "FakeThirdPartyOwner",
        "ciphertext",
        "token",
        "key=",
        "/equip/",
    ] {
        assert!(!text.contains(private));
    }
}

#[tokio::test]
async fn postgres_public_empty_page_keeps_total_and_checks_bounds() {
    let fixture = Fixture::new().await;
    eligible_listing(&fixture, 515).await;
    let result = fixture
        .store
        .list_public(PublicListingQuery { window: window(51, 200), ..public_query() })
        .await
        .unwrap();
    assert_eq!(result.window.offset(), 10_000);
    assert!(result.items.is_empty());
    assert_eq!(result.total, 1);
    assert!(ListingPageWindow::new(Some(52), Some(200)).is_err());
    assert!(ListingPageWindow::new(Some(1), Some(201)).is_err());
    assert!(ListingPageWindow::new(Some(0), Some(20)).is_err());
    assert!(ListingPageWindow::new(Some(1), Some(0)).is_err());
}

struct Replay {
    generate: bool,
    check: bool,
}

impl RegistrationReplayCrypto for Replay {
    fn generate_wrapped(
        &self,
        _: Uuid,
    ) -> Result<WrappedRegistrationReplayKey, ListingCredentialError> {
        if !self.generate {
            return Err(ListingCredentialError);
        }
        WrappedRegistrationReplayKey::new(vec![0x11; 48], [0x22; 12], "fake-data-key".into())
    }

    fn check_wrapped(
        &self,
        replay_key_id: Uuid,
        wrapped: &WrappedRegistrationReplayKey,
    ) -> Result<(), ListingCredentialError> {
        if self.check
            && !replay_key_id.is_nil()
            && wrapped.encryption_key_id() == "fake-data-key"
            && wrapped.wrapped_key() == [0x11; 48]
            && wrapped.nonce() == &[0x22; 12]
        {
            Ok(())
        } else {
            Err(ListingCredentialError)
        }
    }

    fn registration_mac(
        &self,
        _: Uuid,
        _: &WrappedRegistrationReplayKey,
        _: &RegistrationInput<'_>,
    ) -> Result<RegistrationRequestMac, ListingCredentialError> {
        panic!("可用性只检查密钥能力");
    }

    fn rewrap(
        &self,
        _: Uuid,
        _: &WrappedRegistrationReplayKey,
    ) -> Result<WrappedRegistrationReplayKey, ListingCredentialError> {
        panic!("可用性不重封比较钥");
    }
}

#[tokio::test]
async fn postgres_registration_requires_every_dependency_and_never_initializes_rows() {
    use ListingRegistrationAvailability::{Ready, Unavailable};
    let fixture = Fixture::new().await;
    let capable = Replay { generate: true, check: true };
    let status =
        async |crypto: &Replay| fixture.store.registration_availability(crypto).await.unwrap();
    let both = Ready(ListingWorlds { main: true, isekai: true });
    assert_eq!(status(&capable).await, Unavailable);
    fixture
        .sql("UPDATE public.consignment_lookup_control SET worker_ready_until = clock_timestamp()")
        .await;
    assert_eq!(status(&capable).await, Unavailable);
    fixture.sql("UPDATE public.consignment_lookup_control SET worker_ready_until = clock_timestamp() + interval '1 hour'").await;
    assert_eq!(status(&capable).await, both);
    fixture.sql("UPDATE public.consignment_lookup_control SET admitting = false").await;
    assert_eq!(status(&capable).await, Unavailable);
    fixture.sql("UPDATE public.consignment_lookup_control SET admitting = true").await;
    fixture.sql("UPDATE public.worlds SET availability = 'degraded' WHERE id = 'isekai'").await;
    assert_eq!(status(&capable).await, Ready(ListingWorlds { main: true, isekai: false }));
    fixture.sql("UPDATE public.worlds SET enabled = false WHERE id = 'main'").await;
    assert_eq!(status(&capable).await, Unavailable);
    fixture.sql("UPDATE public.worlds SET enabled = true, availability = 'available'").await;
    assert_eq!(status(&Replay { generate: true, check: false }).await, Unavailable);
    assert_eq!(status(&Replay { generate: false, check: true }).await, both);
    fixture.sql("UPDATE public.consignment_replay_keys SET write_current = false").await;
    assert_eq!(status(&Replay { generate: false, check: true }).await, Unavailable);
    assert_eq!(status(&Replay { generate: true, check: false }).await, Unavailable);
    assert_eq!(status(&capable).await, both);
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE write_current)
         FROM public.consignment_replay_keys",
    )
    .fetch_one(fixture.pool())
    .await
    .unwrap();
    assert_eq!(counts, (1, 0));
}

#[tokio::test]
async fn postgres_public_missing_admission_is_unavailable() {
    let fixture = Fixture::new().await;
    let listing = eligible_listing(&fixture, 516).await;
    fixture.sql("DELETE FROM public.consignment_lookup_control WHERE id = 1").await;
    let unavailable = auction_application::consignment::ConsignmentError::Unavailable;
    assert_eq!(fixture.store.list_public(public_query()).await, Err(unavailable));
    assert_eq!(fixture.store.public_detail(listing.id).await, Err(unavailable));
}
