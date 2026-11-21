//! 本人和运营权限、终结时的快照、原命令回执与碰撞排序。

use auction_application::consignment::{
    AdminListingQuery, AdminListingRead, ListingBusinessRejection, ListingEquipmentDetails,
    ListingEquipmentLabel, ListingEquipmentLevel, ListingLookupProgress, ListingPriceDisposition,
    ListingWorld, LookupTransientReason, OwnedListingDecision, OwnedListingRead,
    PrivateListingQuery, PrivateListingViewKind, SellerListingAction, StoredListingDecision,
};
use auction_domain::consignment::{ListingMode, ListingReason, ListingState};
use auction_domain::{CommandId, UserId};
use auction_time::{Duration, Timestamp};
use uuid::Uuid;

use super::fixture::{Fixture, Listing, OTHER, SELLER, batch, private_query, seller, window};

#[tokio::test]
async fn postgres_private_and_admin_scope_filters_and_effective_actions() {
    let fixture = Fixture::new().await;
    let a = Listing::new(&fixture, 506, "42");
    let b = Listing { world: "isekai", mode: "consigned", ..Listing::new(&fixture, 504, "42") };
    let c = Listing {
        seller: OTHER,
        published: fixture.now - Duration::minutes(30),
        ..Listing::new(&fixture, 502, "43")
    };
    for listing in [&a, &b, &c] {
        listing.seed(&fixture, true, None).await;
    }
    fixture.lot("isekai", "42", 1).await;
    let owned = fixture.store.list_owned(seller(), private_query()).await.unwrap();
    assert_eq!(owned.total, 2);
    assert_eq!(
        owned.items.iter().map(|row| row.listing().listing_id).collect::<Vec<_>>(),
        [a.id, b.id]
    );
    assert_eq!(
        owned.items[0].allowed_actions(),
        [SellerListingAction::Reprice, SellerListingAction::Withdraw]
    );
    assert!(owned.items[1].allowed_actions().is_empty());
    assert_eq!(owned.items[1].listing().version.value(), 2);
    assert!(fixture.store.owned_detail(seller(), c.id).await.unwrap().is_none());
    assert!(fixture.store.owned_detail(seller(), Uuid::from_u128(999)).await.unwrap().is_none());
    for (query, expected) in [
        (PrivateListingQuery { world: Some(ListingWorld::Isekai), ..private_query() }, vec![b.id]),
        (PrivateListingQuery { mode: Some(ListingMode::Direct), ..private_query() }, vec![a.id]),
        (PrivateListingQuery { state: Some(ListingState::Closed), ..private_query() }, vec![b.id]),
        (
            PrivateListingQuery { state: Some(ListingState::AwaitingDeposit), ..private_query() },
            vec![],
        ),
    ] {
        let page = fixture.store.list_owned(seller(), query).await.unwrap();
        assert_eq!(page.total, expected.len() as u64);
        assert_eq!(
            page.items.iter().map(|row| row.listing().listing_id).collect::<Vec<_>>(),
            expected
        );
    }
    let empty = fixture
        .store
        .list_owned(seller(), PrivateListingQuery { window: window(3, 1), ..private_query() })
        .await
        .unwrap();
    assert_eq!(empty.total, 2);
    assert!(empty.items.is_empty());
    let admin = |listings, seller_user_id| {
        fixture.store.list_admin(AdminListingQuery { listings, seller_user_id })
    };
    let results = admin(private_query(), None).await.unwrap();
    assert_eq!(results.total, 3);
    assert_eq!(
        results.items.iter().map(|row| row.listing().listing_id).collect::<Vec<_>>(),
        [c.id, a.id, b.id]
    );
    assert_eq!(results.items[0].seller_user_id.as_uuid(), OTHER);
    assert!(results.items[0].can_delist());
    assert!(!results.items[2].can_delist());
    assert_eq!(results.items[2].listing().view.stored_state(), ListingState::AwaitingDeposit);
    assert_eq!(
        results.items[2].listing().view.effective_state(),
        (ListingState::Closed, Some(ListingReason::AuctionLotCreated))
    );
    let filtered = admin(
        PrivateListingQuery {
            world: Some(ListingWorld::Isekai),
            mode: Some(ListingMode::Consigned),
            state: Some(ListingState::Closed),
            ..private_query()
        },
        Some(seller()),
    )
    .await
    .unwrap();
    assert_eq!((filtered.total, filtered.items[0].listing().listing_id), (1, b.id));
    assert_eq!(admin(private_query(), Some(UserId::from_uuid(OTHER))).await.unwrap().total, 1);
    let empty =
        admin(PrivateListingQuery { window: window(4, 1), ..private_query() }, None).await.unwrap();
    assert!(empty.items.is_empty());
    assert_eq!(empty.total, 3);
    assert!(fixture.store.admin_detail(Uuid::from_u128(999)).await.unwrap().is_none());
}

async fn close(fixture: &Fixture, listing: &Listing<'_>, published: bool, admin: bool) -> Uuid {
    let mut tx = fixture.pool().begin().await.unwrap();
    let listing_id = listing.id;
    let state = if published { listing.state() } else { "resolving" };
    let version = if published { 3 } else { 2 };
    let (actor, actor_ref, operation, reason) = if admin {
        ("admin", "Fake operator".to_owned(), "admin_delist", "admin_delisted")
    } else {
        ("public_user", listing.seller.to_string(), "withdraw", "seller_cancelled")
    };
    let now = fixture.now - Duration::minutes(1);
    let command_id = Uuid::new_v4();
    batch(&mut tx, &format!(r#"
        UPDATE public.consignment_listings SET reason_code = '{reason}', state = 'closed',
            state_version = {version}, updated_at = '{now}' WHERE id = '{listing_id}';
        INSERT INTO public.consignment_commands
            (command_id,actor_kind,actor_ref,operation,request_target,request_digest,
             listing_id,result_status,result_code,result_body,created_at)
        VALUES ('{command_id}','{actor}','{actor_ref}','{operation}','{listing_id}',decode(repeat('22',32),'hex'),
                '{listing_id}',200,'ok',jsonb_build_object('command_id','{command_id}','operation','{operation}',
                    'listing_id','{listing_id}','listing_version',{version},'state','closed','price',NULL,
                    'disposition','closed'), '{now}');
        INSERT INTO public.consignment_events
            (listing_id,state_version,command_id,actor_kind,event_kind,from_state,to_state,reason_code,price,occurred_at)
        VALUES ('{listing_id}',{version},'{command_id}','{actor}','closed','{state}','closed','{reason}',101,'{now}');
    "#)).await;
    if admin {
        batch(&mut tx, &format!(r#"
            INSERT INTO public.audit_records
                (id,occurred_at,actor_type,actor_id,action,entity_type,entity_id,result,source,metadata)
            VALUES (gen_random_uuid(),'{now}','admin','Fake operator','consignment.admin_delist',
                    'consignment_listing','{listing_id}','success','admin',
                    jsonb_build_object('command_id','{command_id}','admin_delist_reason','Fake moderation note')),
                    (gen_random_uuid(),'{0}','admin','Fake operator','consignment.admin_delist',
                    'consignment_listing','{listing_id}','rejected','admin',
                    jsonb_build_object('command_id','{command_id}','admin_delist_reason','Fake decoy'));
        "#, fixture.now)).await;
    }
    tx.commit().await.unwrap();
    command_id
}

#[tokio::test]
async fn postgres_private_terminal_snapshots_and_admin_note() {
    let fixture = Fixture::new().await;
    let a = Listing::new(&fixture, 501, "42");
    let b = Listing::new(&fixture, 502, "43");
    a.seed(&fixture, true, None).await;
    b.seed(&fixture, false, None).await;
    close(&fixture, &a, true, true).await;
    close(&fixture, &b, false, false).await;
    fixture.lot("main", "42", 1).await;
    let a_owned = fixture.store.owned_detail(seller(), a.id).await.unwrap().unwrap();
    let b_owned = fixture.store.owned_detail(seller(), b.id).await.unwrap().unwrap();
    assert!(matches!(a_owned.listing().view.kind(), PrivateListingViewKind::Closed {
        reason: ListingReason::AdminDelisted, equipment: Some(equipment),
    } if equipment.label().name == "Fake Blade" && equipment.label().tradeable));
    assert!(matches!(
        b_owned.listing().view.kind(),
        PrivateListingViewKind::Closed { reason: ListingReason::SellerCancelled, equipment: None }
    ));
    let admin = fixture.store.admin_detail(a.id).await.unwrap().unwrap();
    assert_eq!(admin.record.listing(), a_owned.listing());
    assert_eq!(admin.record.admin_delist_reason.unwrap().as_str(), "Fake moderation note");
    assert!(
        fixture
            .store
            .admin_detail(b.id)
            .await
            .unwrap()
            .unwrap()
            .record
            .admin_delist_reason
            .is_none()
    );
    assert!(a_owned.allowed_actions().is_empty());
    assert!(b_owned.allowed_actions().is_empty());
}

async fn seed_lookup_retry(
    fixture: &Fixture,
    listing: &Listing<'_>,
    attempts: i16,
    reason: LookupTransientReason,
) -> Timestamp {
    let next_attempt_at = fixture.now + Duration::minutes(2);
    sqlx::query(
        "UPDATE public.consignment_lookup_work
         SET attempt_count = $2, fence = $2::smallint, last_reason = $3,
             next_attempt_at = $4, updated_at = $5
         WHERE listing_id = $1",
    )
    .bind(listing.id)
    .bind(attempts)
    .bind(reason.as_str())
    .bind(next_attempt_at)
    .bind(fixture.now)
    .execute(fixture.pool())
    .await
    .unwrap();
    next_attempt_at
}

async fn finish_rejected_lookup(
    fixture: &Fixture,
    listing: &Listing<'_>,
    attempts: i16,
    reason: ListingReason,
    snapshot_id: Option<Uuid>,
) {
    let mut tx = fixture.pool().begin().await.unwrap();
    let listing_id = listing.id;
    let now = fixture.now + Duration::minutes(3);
    let deadline = listing.created + Duration::hours(24);
    let seller_user_id = listing.seller;
    let command_id = Uuid::new_v4();
    let reason = reason.as_str();
    sqlx::query(
        "UPDATE public.consignment_listings
         SET state = 'rejected', state_version = 2, reason_code = $2,
             updated_at = $3, current_snapshot_id = $4 WHERE id = $1",
    )
    .bind(listing_id)
    .bind(reason)
    .bind(now)
    .bind(snapshot_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    batch(&mut tx, &format!(r#"
        UPDATE public.consignment_lookup_work SET state = 'done', attempt_count = {attempts},
            fence = {attempts}, next_attempt_at = NULL, completed_at = '{now}', updated_at = '{now}'
        WHERE listing_id = '{listing_id}';
        INSERT INTO public.consignment_commands
            (command_id,actor_kind,actor_ref,operation,request_target,request_digest,
             listing_id,result_status,result_code,result_body,created_at)
        VALUES ('{command_id}','worker','{seller_user_id}','apply_lookup','{listing_id}',
                decode(repeat('22',32),'hex'),'{listing_id}',200,'ok',
                jsonb_build_object('command_id','{command_id}','operation','apply_lookup',
                    'listing_id','{listing_id}','listing_version',2,'state','rejected',
                    'disposition','rejected','price',101,'expires_at','{deadline:?}','reason_code','{reason}'),
                '{now}');
        INSERT INTO public.consignment_events
            (listing_id,state_version,command_id,actor_kind,event_kind,from_state,to_state,reason_code,price,occurred_at)
        VALUES ('{listing_id}',2,'{command_id}','worker','rejected','resolving','rejected','{reason}',101,'{now}');
    "#)).await;
    tx.commit().await.unwrap();
}

async fn assert_lookup_views(
    fixture: &Fixture,
    listing: &Listing<'_>,
    expected: PrivateListingViewKind,
) {
    let owned = fixture.store.owned_detail(seller(), listing.id).await.expect("本人解析视图");
    let admin = fixture.store.admin_detail(listing.id).await.expect("Admin 解析视图");
    assert_eq!(owned.unwrap().listing().view.kind(), &expected, "本人解析视图");
    assert_eq!(admin.unwrap().record.listing().view.kind(), &expected, "Admin 解析视图");
}

#[tokio::test]
async fn postgres_private_lookup_progress_reports_recorded_attempts() {
    let fixture = Fixture::new().await;
    let listing = Listing::new(&fixture, 501, "42");
    listing.seed(&fixture, false, None).await;
    let last_reason = LookupTransientReason::SourceTimeout;
    let next_attempt_at = seed_lookup_retry(&fixture, &listing, 3, last_reason).await;
    let expected = ListingLookupProgress::new(
        3,
        Some(next_attempt_at),
        Some(last_reason),
        false,
        listing.created + Duration::hours(24),
    )
    .unwrap();
    assert_lookup_views(&fixture, &listing, PrivateListingViewKind::Resolving(expected)).await;
}

#[tokio::test]
async fn postgres_private_not_tradeable_keeps_safe_snapshot() {
    let fixture = Fixture::new().await;
    let listing = Listing::new(&fixture, 501, "42");
    listing.seed(&fixture, false, None).await;
    let snapshot_id = Uuid::new_v4();
    let listing_id = listing.id;
    let observed_at = fixture.now;
    fixture.sql(&format!(r#"
        INSERT INTO public.consignment_snapshots
            (id,listing_id,generation,adapter,observed_at,digest,world_id,external_equipment_id,
             name,category,alt_name,level_kind,level_number,tradeable,owner_name,stat_groups)
        VALUES ('{snapshot_id}','{listing_id}',1,'hvdata-equip-v1','{observed_at}',
                decode(repeat('11',32),'hex'),'main','42','Fake Bound Blade','Weapon','Fake Bound Alias',
                'soulbound',NULL,false,'FakeThirdPartyOwner','[]');
    "#)).await;
    let reason = ListingReason::NotTradeable;
    finish_rejected_lookup(&fixture, &listing, 1, reason, Some(snapshot_id)).await;
    let equipment = ListingEquipmentDetails::new(
        observed_at,
        ListingEquipmentLabel {
            name: "Fake Bound Blade".into(),
            category: "Weapon".into(),
            alt_name: Some("Fake Bound Alias".into()),
            level: ListingEquipmentLevel::Soulbound,
            tradeable: false,
        },
        Vec::new(),
    )
    .unwrap();
    assert_lookup_views(
        &fixture,
        &listing,
        PrivateListingViewKind::Ineligible { equipment, reason },
    )
    .await;
}

#[tokio::test]
async fn postgres_private_permanent_lookup_failure_drops_prior_transient_reason() {
    let fixture = Fixture::new().await;
    let listing = Listing::new(&fixture, 501, "42");
    listing.seed(&fixture, false, None).await;
    seed_lookup_retry(&fixture, &listing, 1, LookupTransientReason::SourceTimeout).await;
    let reason = ListingReason::EquipmentNotFound;
    finish_rejected_lookup(&fixture, &listing, 2, reason, None).await;
    let expected = PrivateListingViewKind::LookupFailed { reason, last_lookup_reason: None };
    assert_lookup_views(&fixture, &listing, expected).await;
}

#[tokio::test]
async fn postgres_private_exhausted_lookup_keeps_last_transient_reason() {
    let fixture = Fixture::new().await;
    let listing = Listing::new(&fixture, 501, "42");
    listing.seed(&fixture, false, None).await;
    let last_reason = LookupTransientReason::SourceRateLimited;
    seed_lookup_retry(&fixture, &listing, 5, last_reason).await;
    let reason = ListingReason::LookupExhausted;
    finish_rejected_lookup(&fixture, &listing, 6, reason, None).await;
    let expected =
        PrivateListingViewKind::LookupFailed { reason, last_lookup_reason: Some(last_reason) };
    assert_lookup_views(&fixture, &listing, expected).await;
}

#[tokio::test]
async fn postgres_owned_receipt_preserves_original_decisions_and_actor() {
    let fixture = Fixture::new().await;
    let listing = Listing::new(&fixture, 502, "43");
    let (register, worker) = listing.seed(&fixture, true, None).await;
    let reprice = Uuid::new_v4();
    let rejected = Uuid::new_v4();
    let system = Uuid::new_v4();
    fixture.sql(&format!(r#"
        INSERT INTO public.consignment_commands
            (command_id,actor_kind,actor_ref,operation,request_target,request_digest,
             listing_id,result_status,result_code,result_body,created_at)
        SELECT command_id, actor, '{SELLER}', operation, '{listing_id}', decode(repeat('22',32),'hex'),
               '{listing_id}', status, code, jsonb_build_object('command_id',command_id,
                   'operation',operation,'listing_id','{listing_id}','listing_version',version,
                   'state',state,'price',price,'disposition',disposition), '{now}'
        FROM (VALUES ('{reprice}'::uuid,'public_user','reprice',200,'ok',2,'displayed',101,'unchanged'),
                     ('{rejected}'::uuid,'public_user','reprice',409,'stale_version',NULL,NULL,NULL,'rejected'),
                     ('{system}'::uuid,'system','auto_close',200,'ok',NULL,NULL,NULL,'unchanged'))
             AS rows(command_id,actor,operation,status,code,version,state,price,disposition)
    "#, listing_id = listing.id, now = fixture.now)).await;
    let withdraw = close(&fixture, &listing, true, false).await;
    let command = |uuid| fixture.store.owned_command(seller(), CommandId::from_uuid(uuid));
    let original = command(register).await.unwrap().unwrap();
    assert_eq!(original.command_id.as_uuid(), register);
    assert_eq!(original.decision.status_and_code(), (202, "accepted"));
    assert!(
        matches!(original.decision, OwnedListingDecision::Register(StoredListingDecision::Accepted(receipt))
        if receipt.listing_id() == listing.id && receipt.state() == ListingState::Resolving && receipt.price.value() == 101)
    );
    assert!(matches!(command(reprice).await.unwrap().unwrap().decision,
        OwnedListingDecision::Reprice(StoredListingDecision::Accepted(receipt))
        if receipt.version.value() == 2 && receipt.price.value() == 101 &&
           receipt.state() == ListingState::Displayed && receipt.disposition == ListingPriceDisposition::Unchanged));
    assert!(matches!(
        command(rejected).await.unwrap().unwrap().decision,
        OwnedListingDecision::Reprice(StoredListingDecision::Rejected(
            ListingBusinessRejection::StaleVersion
        ))
    ));
    assert!(matches!(command(withdraw).await.unwrap().unwrap().decision,
        OwnedListingDecision::Withdraw(StoredListingDecision::Accepted(receipt))
        if receipt.version.value() == 3 && receipt.price.is_none()));
    assert!(
        fixture
            .store
            .owned_command(UserId::from_uuid(OTHER), CommandId::from_uuid(register))
            .await
            .unwrap()
            .is_none()
    );
    for uuid in [worker.unwrap(), system, Uuid::from_u128(999)] {
        assert!(command(uuid).await.unwrap().is_none());
    }
}

#[tokio::test]
async fn postgres_admin_conflicts_sort_and_truncate_only_after_50() {
    let fixture = Fixture::new().await;
    let listing = Listing::new(&fixture, 502, "43");
    listing.seed(&fixture, true, None).await;
    let command_id = Uuid::new_v4();
    let listing_id = listing.id;
    let now = fixture.now;
    fixture.sql(&format!(r#"
        INSERT INTO public.consignment_commands
            (command_id,actor_kind,actor_ref,operation,request_target,request_digest,
             listing_id,result_status,result_code,result_body,created_at)
        VALUES ('{command_id}','system','consignment','auto_close','{listing_id}',decode(repeat('22',32),'hex'),
                '{listing_id}',200,'ok',jsonb_build_object('command_id','{command_id}','operation','auto_close',
                    'listing_id','{listing_id}','listing_version',NULL,'state',NULL,'price',NULL,
                    'disposition','conflicts_recorded'), '{now}')
    "#)).await;
    let mut lots = Vec::new();
    for seq in 1..=51 {
        let lot = fixture.lot("main", "00043", seq).await;
        lots.push(lot);
    }
    let link = async |lot, time| {
        fixture.sql(&format!(
        "INSERT INTO public.consignment_listing_auction_conflicts (listing_id,lot_id,detected_at,command_id)
         VALUES ('{listing_id}','{lot}','{time}','{command_id}')")).await;
    };
    let detail = fixture.store.admin_detail(listing.id).await.unwrap().unwrap();
    assert!(!detail.conflicts_truncated);
    assert!(detail.conflicts.is_empty());
    // ID 小的较新碰撞必须排最前；其余同刻按 ID 降序。
    link(lots[0], now + Duration::seconds(1)).await;
    for lot in &lots[1..50] {
        link(*lot, now).await;
    }
    let fifty = fixture.store.admin_detail(listing.id).await.unwrap().unwrap();
    assert!(!fifty.conflicts_truncated);
    assert_eq!(fifty.conflicts.len(), 50);
    let expected: Vec<_> =
        std::iter::once(lots[0]).chain(lots[1..50].iter().rev().copied()).collect();
    assert_eq!(fifty.conflicts.iter().map(|c| c.lot_id.as_uuid()).collect::<Vec<_>>(), expected);
    link(lots[50], now - Duration::seconds(1)).await;
    let truncated = fixture.store.admin_detail(listing.id).await.unwrap().unwrap();
    assert!(truncated.conflicts_truncated);
    assert_eq!(truncated.conflicts, fifty.conflicts);
}

#[tokio::test]
async fn postgres_private_and_admin_count_and_page_share_snapshot() {
    let fixture = Fixture::new().await;
    let listing = Listing::new(&fixture, 520, "42");
    listing.seed(&fixture, true, None).await;
    let mut lock = fixture.pool().begin().await.unwrap();
    batch(&mut lock, "LOCK public.consignment_snapshots IN ACCESS EXCLUSIVE MODE").await;
    let blocker: i32 =
        sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *lock).await.unwrap();
    let query = PrivateListingQuery { state: Some(ListingState::Displayed), ..private_query() };
    let admin_query = AdminListingQuery { listings: query, seller_user_id: None };
    let owned = fixture.store.clone();
    let admin = fixture.store.clone();
    let owned = tokio::spawn(async move { owned.list_owned(seller(), query).await });
    let admin = tokio::spawn(async move { admin.list_admin(admin_query).await });
    // 只有取页语句关联快照表。两条读连接都被锁挡住时，计数已经执行完毕。
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let blocked: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_catalog.pg_stat_activity
                 WHERE datname = current_database() AND $1 = ANY(pg_blocking_pids(pid))",
            )
            .bind(blocker)
            .fetch_one(fixture.pool())
            .await
            .unwrap();
            if blocked == 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("两条分页语句等待快照表锁");
    fixture
        .sql("UPDATE public.consignment_lookup_control SET admitting = false, enabled_epoch = 2")
        .await;
    lock.commit().await.unwrap();
    let owned = owned.await.unwrap().unwrap();
    let admin = admin.await.unwrap().unwrap();
    assert_eq!((owned.total, owned.items.len()), (1, 1));
    assert_eq!((admin.total, admin.items.len()), (1, 1));
    assert_eq!(owned.items[0].listing().view.effective_state().0, ListingState::Displayed);
    assert_eq!(admin.items[0].listing().view.effective_state().0, ListingState::Displayed);
    assert_eq!(fixture.store.list_owned(seller(), query).await.unwrap().total, 0);
    assert_eq!(fixture.store.list_admin(admin_query).await.unwrap().total, 0);
}
