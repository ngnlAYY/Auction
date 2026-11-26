//! 登记仲裁、准入、额度顺序及凭证原子性。

use auction_application::consignment::RegistrationSeller;
use auction_application::consignment::{
    ConsignmentError, ListingBusinessRejection as Rejection, ListingCommandOutcome,
    ListingCommandRejection, ListingCommandStore, StoredListingDecision,
    UnrecordedListingRejection,
};
use auction_domain::EquipmentKey;
use auction_domain::consignment::{FeeBps, ListingMode, ListingSellerName};
use uuid::Uuid;

use super::fixture::{
    Crypto, FAKE_KEY, Fixture, OTHER, OTHER_FAKE_KEY, SELLER, apply, expire_listing, gear, new_id,
    new_internal_command_id, observation, register, withdraw,
};

#[tokio::test]
async fn postgres_register_replays_before_disable_or_fee_and_uses_aad_once() {
    let f = Fixture::new().await;
    let identity = gear("isekai", "420");
    let command_id = new_id();
    let listing_id = new_id();
    let original =
        register(command_id, listing_id, &identity, &f.key, SELLER, ListingMode::Consigned);
    let aad = original.aad_equipment_id;
    let work_id = original.work_id;
    let receipt = f
        .store
        .register(original, Some(&f.crypto), Some(&f.crypto))
        .await
        .unwrap()
        .into_result()
        .unwrap();

    let sealed_aad: Uuid = sqlx::query_scalar(
        "SELECT aad_equipment_id FROM public.consignment_registration_secrets WHERE listing_id=$1",
    )
    .bind(listing_id)
    .fetch_one(f.pool())
    .await
    .unwrap();
    assert_eq!(sealed_aad, aad.as_uuid());
    assert_ne!(sealed_aad, listing_id);
    let persisted: (String, i16, String) = sqlx::query_as(
        "SELECT l.world_id, l.fee_bps, w.state FROM public.consignment_listings l
         JOIN public.consignment_lookup_work w ON w.listing_id=l.id WHERE w.id=$1",
    )
    .bind(work_id)
    .fetch_one(f.pool())
    .await
    .unwrap();
    assert_eq!(persisted, ("isekai".into(), 500, "queued".into()));
    f.store
        .withdraw(withdraw(new_id(), listing_id, SELLER, 1))
        .await
        .unwrap()
        .into_result()
        .unwrap();
    assert!(f.retired(listing_id).await);

    let new_command = new_id();
    for enabled in [false, true] {
        let configured_fee = FeeBps::new(321).unwrap();
        let mut replay =
            register(command_id, new_id(), &identity, &f.key, SELLER, ListingMode::Consigned);
        replay.policy.enabled = enabled;
        replay.policy.consigned_fee_bps = configured_fee;
        assert_eq!(
            f.store.register(replay, None, Some(&f.crypto)).await.unwrap(),
            ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(receipt))
        );
        let wrong_key = EquipmentKey::new(OTHER_FAKE_KEY).unwrap();
        let mut mismatch =
            register(command_id, new_id(), &identity, &wrong_key, SELLER, ListingMode::Consigned);
        mismatch.policy.enabled = enabled;
        assert_eq!(
            f.store.register(mismatch, None, Some(&f.crypto)).await.unwrap(),
            ListingCommandOutcome::Rejected(ListingCommandRejection::Unrecorded(
                UnrecordedListingRejection::CommandIdConflict
            ))
        );
        let mut fresh =
            register(new_command, new_id(), &identity, &f.key, SELLER, ListingMode::Consigned);
        fresh.policy.enabled = enabled;
        fresh.policy.consigned_fee_bps = configured_fee;
        let result =
            f.store.register(fresh, Some(&f.crypto), Some(&f.crypto)).await.unwrap().into_result();
        if enabled {
            assert_eq!(result, Err(Rejection::FeeChanged.into()));
            assert_eq!(f.command_count(new_command).await, 1);
        } else {
            assert_eq!(result, Err(ConsignmentError::Disabled));
            assert_eq!(f.command_count(new_command).await, 0);
        }
    }
    assert_eq!(*f.crypto.seals.lock().unwrap(), [aad]);
    assert_eq!(
        f.audit(command_id).await,
        [("public_user".into(), "consignment.register".into(), "success".into())]
    );
    assert_eq!(
        f.audit(new_command).await,
        [("public_user".into(), "consignment.register".into(), "rejected".into())]
    );
    let fixture_url_shape =
        ["/isekai/equip/", identity.external_equipment_id(), "/", FAKE_KEY].concat();
    let snapshot = f.persisted().await.to_string();
    assert!(!snapshot.contains(FAKE_KEY), "寄售事实不得持久化假明文夹具");
    assert!(!snapshot.contains(&fixture_url_shape), "寄售事实不得持久化假装备链接");
}

#[tokio::test]
async fn postgres_register_seal_mac_and_readiness_failure_leave_no_partial_facts() {
    let f = Fixture::new().await;
    let g = gear("main", "421");
    let failing_sealer = Crypto { fail_seal: true, ..Crypto::default() };
    let failing_mac = Crypto { fail_mac: true, ..Crypto::default() };
    let initial = f.persisted().await;
    let command_id = new_id();
    let listing_id = new_id();
    let fresh = || register(command_id, listing_id, &g, &f.key, SELLER, ListingMode::Direct);
    for (sealer, crypto) in [
        (None, Some(&f.crypto)),
        (Some(&f.crypto), None),
        (Some(&failing_sealer), Some(&f.crypto)),
        (Some(&f.crypto), Some(&failing_mac)),
    ] {
        assert_eq!(
            f.store.register(fresh(), sealer.map(|s| s as _), crypto.map(|c| c as _)).await,
            Err(ConsignmentError::Unavailable)
        );
        assert!(f.persisted().await == initial, "能力失败须回滚比较钥在内的所有写入");
    }
    assert!(f.crypto.seals.lock().unwrap().is_empty(), "MAC 失败不得调用封装");
    assert_eq!(failing_sealer.seals.lock().unwrap().len(), 1);

    sqlx::query(
        "UPDATE public.consignment_lookup_control SET worker_ready_until=statement_timestamp()",
    )
    .execute(f.pool())
    .await
    .unwrap();
    let unavailable = f.persisted().await;
    assert_eq!(
        f.store.register(fresh(), Some(&f.crypto), Some(&f.crypto)).await,
        Err(ConsignmentError::Unavailable)
    );
    assert!(f.persisted().await == unavailable, "相关持久事实须保持一致");
    sqlx::query("UPDATE public.consignment_lookup_control SET worker_ready_until=statement_timestamp()+interval '1 minute'")
        .execute(f.pool()).await.unwrap();
    let accepted = f
        .store
        .register(fresh(), Some(&f.crypto), Some(&f.crypto))
        .await
        .unwrap()
        .into_result()
        .unwrap();
    assert_eq!(accepted.listing_id(), listing_id);
    assert_eq!(
        f.store.register(fresh(), Some(&failing_sealer), Some(&f.crypto)).await.unwrap(),
        ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(accepted))
    );
    assert_eq!(failing_sealer.seals.lock().unwrap().len(), 1);
    assert_eq!(
        f.store.register(fresh(), None, Some(&failing_mac)).await,
        Err(ConsignmentError::Unavailable)
    );
    assert_eq!(f.command_count(command_id).await, 1);
}

#[tokio::test]
async fn postgres_register_concurrent_replay_duplicate_and_single_current_key() {
    let f = Fixture::new().await;
    let g = gear("main", "422");
    let command_id = new_id();
    let first = register(command_id, new_id(), &g, &f.key, SELLER, ListingMode::Direct);
    let repeat = register(command_id, new_id(), &g, &f.key, SELLER, ListingMode::Direct);
    let blocker = f.block_control().await;
    let (first, repeat, ()) = tokio::join!(
        f.store.register(first, Some(&f.crypto), Some(&f.crypto)),
        f.store.register(repeat, Some(&f.crypto), Some(&f.crypto)),
        f.release_when_waiting(blocker, 2),
    );
    let a = first.unwrap();
    let b = repeat.unwrap();
    assert!(matches!(
        (&a, &b),
        (ListingCommandOutcome::Accepted(_), ListingCommandOutcome::Replayed(_))
            | (ListingCommandOutcome::Replayed(_), ListingCommandOutcome::Accepted(_))
    ));
    assert_eq!(a.into_result().unwrap(), b.into_result().unwrap());
    let duplicate = register(new_id(), new_id(), &g, &f.key, SELLER, ListingMode::Consigned);
    assert_eq!(
        f.store.register(duplicate, Some(&f.crypto), Some(&f.crypto)).await.unwrap().into_result(),
        Err(Rejection::AlreadyOpen.into())
    );

    let competing = gear("main", "426");
    let a = register(new_id(), new_id(), &competing, &f.key, SELLER, ListingMode::Direct);
    let b = register(new_id(), new_id(), &competing, &f.key, SELLER, ListingMode::Consigned);
    let blocker = f.block_control().await;
    let (a, b, ()) = tokio::join!(
        f.store.register(a, Some(&f.crypto), Some(&f.crypto)),
        f.store.register(b, Some(&f.crypto), Some(&f.crypto)),
        f.release_when_waiting(blocker, 2),
    );
    assert!(matches!(
        (a.unwrap().into_result(), b.unwrap().into_result()),
        (Ok(_), Err(ConsignmentError::Business(Rejection::AlreadyOpen)))
            | (Err(ConsignmentError::Business(Rejection::AlreadyOpen)), Ok(_))
    ));

    // 第二个空数据库上两个不同卖家同时初始化比较钥，两个请求都要在同一当前钥上成功。
    let f = Fixture::new().await;
    let first = register(new_id(), new_id(), &g, &f.key, SELLER, ListingMode::Direct);
    let other = gear("isekai", "422");
    let second = register(new_id(), new_id(), &other, &f.key, OTHER, ListingMode::Direct);
    let blocker = f.block_control().await;
    let (a, b, ()) = tokio::join!(
        f.store.register(first, Some(&f.crypto), Some(&f.crypto)),
        f.store.register(second, Some(&f.crypto), Some(&f.crypto)),
        f.release_when_waiting(blocker, 2),
    );
    assert!(matches!(a.unwrap(), ListingCommandOutcome::Accepted(_)));
    assert!(matches!(b.unwrap(), ListingCommandOutcome::Accepted(_)));
    let count: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.consignment_replay_keys WHERE write_current),
                (SELECT count(DISTINCT replay_key_id) FROM public.consignment_commands)",
    )
    .fetch_one(f.pool())
    .await
    .unwrap();
    assert_eq!(count, (1, 1));
}

#[tokio::test]
async fn postgres_register_priority_and_effective_cross_world_limits() {
    let f = Fixture::new().await;
    let listing_id = new_id();
    let g = gear("main", "423");
    let other_gear = gear("isekai", "423");
    f.registered(listing_id, &g, SELLER, ListingMode::Direct).await;
    f.registered(new_id(), &other_gear, OTHER, ListingMode::Direct).await;

    let attempt = |id, seller, equipment, hour| {
        let mut cmd = register(id, new_id(), equipment, &f.key, seller, ListingMode::Consigned);
        cmd.policy.max_open_per_user = 1;
        cmd.policy.registrations_per_hour = hour;
        f.store.register(cmd, Some(&f.crypto), Some(&f.crypto))
    };
    assert_eq!(
        attempt(new_id(), SELLER, &g, 1).await.unwrap().into_result(),
        Err(Rejection::AlreadyOpen.into())
    );
    assert_eq!(
        attempt(new_id(), SELLER, &other_gear, 1).await.unwrap().into_result(),
        Err(Rejection::RegistrationRateLimited.into())
    );
    assert_eq!(
        attempt(new_id(), SELLER, &other_gear, 2).await.unwrap().into_result(),
        Err(Rejection::ListingLimitReached.into())
    );

    expire_listing(&f, listing_id).await;
    assert_eq!(
        attempt(new_id(), SELLER, &other_gear, 1).await.unwrap().into_result(),
        Err(Rejection::RegistrationRateLimited.into())
    );
    assert!(attempt(new_id(), SELLER, &other_gear, 2).await.unwrap().into_result().is_ok());

    // 允许当前持久占用过期后直接复用同件名额。
    let replaced = f.registered(new_id(), &g, SELLER, ListingMode::Direct).await;
    assert!(!replaced.is_nil());
    assert_eq!(
        f.state(listing_id).await,
        ("expired".into(), 2, Some("lookup_deadline".into()), false)
    );
    assert!(f.retired(listing_id).await);

    f.lot("000423", 1).await;
    f.set_blacklist(SELLER).await;
    let rejection_id = new_id();
    assert_eq!(
        attempt(rejection_id, SELLER, &g, 1).await.unwrap().into_result(),
        Err(Rejection::ParticipantIneligible.into())
    );
    sqlx::query("UPDATE public.users SET status='active' WHERE id=$1")
        .bind(SELLER)
        .execute(f.owner.postgres_pool())
        .await
        .unwrap();
    assert_eq!(
        attempt(rejection_id, SELLER, &g, 1).await.unwrap(),
        ListingCommandOutcome::Replayed(StoredListingDecision::Rejected(
            Rejection::ParticipantIneligible
        ))
    );
    assert_eq!(
        attempt(new_id(), SELLER, &g, 1).await.unwrap().into_result(),
        Err(Rejection::EquipmentBusy.into())
    );
}

#[tokio::test]
async fn postgres_anonymous_name_quota_includes_same_name_account_one_way() {
    // 不变量 2、6：匿名按名额度计入同名本站账户的挂单，账户额度不计入同名匿名挂单。
    let f = Fixture::new().await;
    f.registered(new_id(), &gear("main", "431"), SELLER, ListingMode::Direct).await;
    // SELLER 的 username 是 FakeWriteSeller；大小写不同仍是同一角色名。
    let name = ListingSellerName::new("fakewriteseller").unwrap();
    let anonymous_gear = gear("isekai", "431");
    let anonymous = |hourly, open| {
        let mut cmd =
            register(new_id(), new_id(), &anonymous_gear, &f.key, SELLER, ListingMode::Direct);
        cmd.input.seller = RegistrationSeller::GameName(&name);
        cmd.policy.registrations_per_hour = hourly;
        cmd.policy.max_open_per_user = open;
        f.store.register(cmd, Some(&f.crypto), Some(&f.crypto))
    };

    // 正例：同名账户已占满每小时或活动额度时，免登录登记同名被拒。
    assert_eq!(
        anonymous(1, 2).await.unwrap().into_result(),
        Err(Rejection::RegistrationRateLimited.into())
    );
    assert_eq!(
        anonymous(2, 1).await.unwrap().into_result(),
        Err(Rejection::ListingLimitReached.into())
    );

    // 反例：未达上限时受理；随后账户在同一额度下仍可登记，匿名挂单不占账户额度。
    assert!(anonymous(2, 2).await.unwrap().into_result().is_ok());
    let account_gear = gear("main", "432");
    let mut account =
        register(new_id(), new_id(), &account_gear, &f.key, SELLER, ListingMode::Direct);
    account.policy.registrations_per_hour = 2;
    account.policy.max_open_per_user = 2;
    assert!(
        f.store
            .register(account, Some(&f.crypto), Some(&f.crypto))
            .await
            .unwrap()
            .into_result()
            .is_ok()
    );
}

#[tokio::test]
async fn postgres_register_published_occupancy_beats_limits_and_world_gate() {
    let f = Fixture::new().await;
    let g = gear("main", "424");
    let listing_id = new_id();
    let work_id = f.registered(listing_id, &g, SELLER, ListingMode::Direct).await;
    let fence = f.grant(listing_id, work_id, 1, 30).await;
    f.store
        .apply_lookup(apply(
            new_internal_command_id(),
            fence,
            observation(&g, &f.key, "FakeWriteSeller", true),
            1,
        ))
        .await
        .unwrap()
        .into_result()
        .unwrap();

    let other_listing = new_id();
    f.registered(other_listing, &gear("isekai", "999"), OTHER, ListingMode::Direct).await;
    let mut cmd = register(new_id(), new_id(), &g, &f.key, OTHER, ListingMode::Direct);
    cmd.policy.registrations_per_hour = 1;
    cmd.policy.max_open_per_user = 1;
    assert_eq!(
        f.store.register(cmd, Some(&f.crypto), Some(&f.crypto)).await.unwrap().into_result(),
        Err(Rejection::EquipmentBusy.into())
    );
    let unavailable_gear = gear("isekai", "998");
    let mut unavailable =
        register(new_id(), new_id(), &unavailable_gear, &f.key, SELLER, ListingMode::Direct);
    unavailable.policy.worlds.isekai = false;
    assert_eq!(
        f.store
            .register(unavailable, Some(&f.crypto), Some(&f.crypto))
            .await
            .unwrap()
            .into_result(),
        Err(Rejection::WorldUnavailable.into())
    );
}

#[tokio::test]
async fn postgres_register_public_bootstrap_initializes_once_and_disabled_writes_nothing() {
    let f = Fixture::new().await;
    sqlx::query("UPDATE public.consignment_lookup_control SET admitting=false")
        .execute(f.pool())
        .await
        .unwrap();
    let before = f.persisted().await;
    assert_eq!(
        f.store.initialize_listing_replay_key(new_id(), &f.crypto).await,
        Err(ConsignmentError::Unavailable)
    );
    let g = gear("main", "425");
    let cmd = register(new_id(), new_id(), &g, &f.key, SELLER, ListingMode::Direct);
    assert_eq!(
        f.store.register(cmd, Some(&f.crypto), Some(&f.crypto)).await,
        Err(ConsignmentError::Unavailable)
    );
    assert!(f.persisted().await == before, "相关持久事实须保持一致");
    sqlx::query("UPDATE public.consignment_lookup_control SET admitting=true")
        .execute(f.pool())
        .await
        .unwrap();

    let first_id = new_id();
    f.store.initialize_listing_replay_key(first_id, &f.crypto).await.unwrap();
    let initialized = f.persisted().await;
    f.store.initialize_listing_replay_key(new_id(), &f.crypto).await.unwrap();
    assert!(f.persisted().await == initialized, "相关持久事实须保持一致");
    let id: Uuid =
        sqlx::query_scalar("SELECT id FROM public.consignment_replay_keys WHERE write_current")
            .fetch_one(f.pool())
            .await
            .unwrap();
    assert_eq!(id, first_id);
}

#[tokio::test]
async fn postgres_register_concurrent_worlds_share_seller_hourly_budget() {
    let f = Fixture::new().await;
    let main = gear("main", "427");
    let isekai = gear("isekai", "427");
    let mut first = register(new_id(), new_id(), &main, &f.key, SELLER, ListingMode::Direct);
    let mut second = register(new_id(), new_id(), &isekai, &f.key, SELLER, ListingMode::Consigned);
    first.policy.registrations_per_hour = 1;
    second.policy.registrations_per_hour = 1;
    let blocker = f.block_control().await;
    let (first, second, ()) = tokio::join!(
        f.store.register(first, Some(&f.crypto), Some(&f.crypto)),
        f.store.register(second, Some(&f.crypto), Some(&f.crypto)),
        f.release_when_waiting(blocker, 2),
    );
    assert!(matches!(
        (first.unwrap().into_result(), second.unwrap().into_result()),
        (Ok(_), Err(ConsignmentError::Business(Rejection::RegistrationRateLimited)))
            | (Err(ConsignmentError::Business(Rejection::RegistrationRateLimited)), Ok(_))
    ));
}

#[tokio::test]
async fn postgres_register_concurrent_worlds_share_seller_active_budget() {
    let f = Fixture::new().await;
    let main = gear("main", "428");
    let isekai = gear("isekai", "428");
    let mut first = register(new_id(), new_id(), &main, &f.key, SELLER, ListingMode::Direct);
    let mut second = register(new_id(), new_id(), &isekai, &f.key, SELLER, ListingMode::Consigned);
    first.policy.max_open_per_user = 1;
    second.policy.max_open_per_user = 1;
    let blocker = f.block_control().await;
    let (first, second, ()) = tokio::join!(
        f.store.register(first, Some(&f.crypto), Some(&f.crypto)),
        f.store.register(second, Some(&f.crypto), Some(&f.crypto)),
        f.release_when_waiting(blocker, 2),
    );
    assert!(matches!(
        (first.unwrap().into_result(), second.unwrap().into_result()),
        (Ok(_), Err(ConsignmentError::Business(Rejection::ListingLimitReached)))
            | (Err(ConsignmentError::Business(Rejection::ListingLimitReached)), Ok(_))
    ));
}

#[tokio::test]
async fn postgres_register_rejects_unavailable_database_world_even_when_config_allows_it() {
    let f = Fixture::new().await;
    let g = gear("main", "424");
    for (enabled, availability) in [(false, "available"), (true, "unavailable")] {
        sqlx::query("UPDATE public.worlds SET enabled=$1, availability=$2 WHERE id='main'")
            .bind(enabled)
            .bind(availability)
            .execute(f.owner.postgres_pool())
            .await
            .unwrap();
        let cmd = register(new_id(), new_id(), &g, &f.key, SELLER, ListingMode::Direct);
        assert!(cmd.policy.worlds.main);
        let command_id = cmd.command_id.as_uuid();
        assert_eq!(
            f.store.register(cmd, Some(&f.crypto), Some(&f.crypto)).await.unwrap(),
            ListingCommandOutcome::Rejected(ListingCommandRejection::Recorded(
                Rejection::WorldUnavailable
            ))
        );
        assert_eq!(f.command_count(command_id).await, 1);
    }
    assert!(f.crypto.seals.lock().unwrap().is_empty());
}
