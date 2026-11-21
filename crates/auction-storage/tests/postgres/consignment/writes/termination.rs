//! 强制终止优先序、真实证据、碰撞差集与提交时故障。

use auction_application::consignment::{
    AutoCloseEvidence, ConsignmentError, ListingAutoCloseEffect,
    ListingBusinessRejection as Rejection, ListingCommandOutcome, ListingCommandStore,
    ListingLookupEffect, StoredListingDecision,
};
use auction_domain::LotId;
use auction_domain::consignment::{ListingMode, ListingReason, ListingState};
use uuid::Uuid;

use super::fixture::{
    Fixture, OTHER, SELLER, apply, auto, expire_listing, gear, new_id, observation, reprice,
    withdraw,
};

#[tokio::test]
async fn postgres_auto_close_rechecks_evidence_then_records_collision_set_and_one_version() {
    let f = Fixture::new().await;
    let g = gear("main", "451");
    let listing_id = new_id();
    f.registered(listing_id, &g, SELLER, ListingMode::Direct).await;
    let first_id = new_id();
    let first_lot = new_id();
    let close = auto(
        first_id,
        listing_id,
        AutoCloseEvidence::AuctionLotCreated(LotId::from_uuid(first_lot)),
    );
    let before = f.persisted().await;
    assert_eq!(
        f.store.auto_close(close).await.unwrap().into_result(),
        Err(ConsignmentError::ConditionNotMet)
    );
    assert!(f.persisted().await == before, "相关持久事实须保持一致");

    f.lot_with_id("000451", 1, first_lot).await;
    let second_lot = f.lot("451", 2).await;
    let accepted = f.store.auto_close(close).await.unwrap().into_result().unwrap();
    assert!(
        matches!(accepted, ListingAutoCloseEffect::Terminated(projection) if projection.reason == Some(ListingReason::AuctionLotCreated))
    );
    assert!(f.retired(listing_id).await);
    let conflicts = || async {
        sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT lot_id,command_id FROM public.consignment_listing_auction_conflicts
             WHERE listing_id=$1 ORDER BY lot_id",
        )
        .bind(listing_id)
        .fetch_all(f.pool())
        .await
        .unwrap()
    };
    let mut expected = [(first_lot, first_id), (second_lot, first_id)];
    expected.sort_unstable();
    assert_eq!(conflicts().await, expected);
    assert_eq!(
        f.state(listing_id).await,
        ("closed".into(), 2, Some("auction_lot_created".into()), false)
    );
    assert_eq!(
        f.store.auto_close(close).await.unwrap(),
        ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(accepted))
    );
    let conflict = auto(
        first_id,
        listing_id,
        AutoCloseEvidence::AuctionLotCreated(LotId::from_uuid(second_lot)),
    );
    assert_eq!(
        f.store.auto_close(conflict).await.unwrap().into_result(),
        Err(ConsignmentError::CommandIdConflict)
    );

    let observed = f.lot("0000451", 3).await;
    let unseen = f.lot("00000451", 4).await;
    let append_id = new_id();
    let append = auto(
        append_id,
        listing_id,
        AutoCloseEvidence::AuctionLotCreated(LotId::from_uuid(observed)),
    );
    assert_eq!(
        f.store.auto_close(append).await.unwrap().into_result(),
        Ok(ListingAutoCloseEffect::ConflictsRecorded)
    );
    assert_eq!(
        f.store.auto_close(append).await.unwrap(),
        ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(
            ListingAutoCloseEffect::ConflictsRecorded
        ))
    );
    let mut expected = expected.to_vec();
    expected.push((observed, append_id));
    expected.sort_unstable();
    assert_eq!(conflicts().await, expected);
    assert!(!conflicts().await.iter().any(|(lot_id, _)| *lot_id == unseen));
    assert_eq!(f.state(listing_id).await.1, 2);
    assert_eq!(
        f.store.auto_close(close).await.unwrap(),
        ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(accepted))
    );
    let associated = auto(
        new_id(),
        listing_id,
        AutoCloseEvidence::AuctionLotCreated(LotId::from_uuid(second_lot)),
    );
    assert_eq!(
        f.store.auto_close(associated).await.unwrap().into_result(),
        Ok(ListingAutoCloseEffect::Unchanged)
    );
}

#[tokio::test]
async fn postgres_forced_closure_reason_precedence_and_ownership_are_atomic() {
    let f = Fixture::new().await;
    let mut listings = Vec::new();
    for (eid, seller) in [("452", SELLER), ("453", SELLER), ("454", OTHER), ("455", OTHER)] {
        let listing_id = new_id();
        f.registered(listing_id, &gear("main", eid), seller, ListingMode::Direct).await;
        expire_listing(&f, listing_id).await;
        listings.push(listing_id);
    }
    let collision = f.lot("000452", 1).await;
    let fact = f.set_blacklist(SELLER).await;
    // 解禁后的新增历史仍关闭旧挂单。
    sqlx::query("UPDATE public.users SET status='active' WHERE id=$1")
        .bind(SELLER)
        .execute(f.owner.postgres_pool())
        .await
        .unwrap();
    for ((listing_id, seller, reason, state), stop) in [
        ((listings[0], SELLER, "auction_lot_created", "closed"), true),
        ((listings[1], SELLER, "seller_ineligible", "closed"), true),
        ((listings[2], OTHER, "service_disabled", "closed"), true),
        ((listings[3], OTHER, "lookup_deadline", "expired"), false),
    ] {
        sqlx::query(
            "UPDATE public.consignment_lookup_control SET admitting=NOT $1, enabled_epoch=CASE WHEN $1 THEN 2 ELSE 1 END",
        ).bind(stop).execute(f.pool()).await.unwrap();
        let cmd_id = new_id();
        let wrong_seller = if seller == SELLER { OTHER } else { SELLER };
        assert_eq!(
            f.store
                .reprice(reprice(new_id(), listing_id, wrong_seller, 1, 900))
                .await
                .unwrap()
                .into_result(),
            Err(Rejection::ListingNotFound.into())
        );
        assert_eq!(f.state(listing_id).await.1, 1);
        let cmd = reprice(cmd_id, listing_id, seller, 2, 900);
        assert_eq!(
            f.store.reprice(cmd).await.unwrap().into_result(),
            Err(Rejection::ListingNotActive.into())
        );
        assert_eq!(f.state(listing_id).await, (state.into(), 2, Some(reason.into()), false));
        assert!(f.retired(listing_id).await);
        let audit = f.audit(cmd_id).await;
        assert_eq!(audit.len(), 2);
        assert!(audit.iter().any(|(actor, _, result)| actor == "system" && result == "success"));
        assert!(audit.contains(&(
            "public_user".into(),
            "consignment.reprice".into(),
            "rejected".into()
        )));
        assert_eq!(
            f.store.reprice(cmd).await.unwrap(),
            ListingCommandOutcome::Replayed(StoredListingDecision::Rejected(
                Rejection::ListingNotActive
            ))
        );
    }
    let coupled: Uuid = sqlx::query_scalar(
        "SELECT lot_id FROM public.consignment_listing_auction_conflicts WHERE listing_id=$1",
    )
    .bind(listings[0])
    .fetch_one(f.pool())
    .await
    .unwrap();
    assert_eq!(coupled, collision);
    let terminal = auto(new_id(), listings[1], AutoCloseEvidence::SellerIneligible(fact));
    assert_eq!(
        f.store.auto_close(terminal).await.unwrap().into_result(),
        Ok(ListingAutoCloseEffect::Unchanged)
    );
}

#[tokio::test]
async fn postgres_forced_close_and_user_rejection_roll_back_on_commit_or_audit_error() {
    let f = Fixture::new().await;
    let listing_id = new_id();
    f.registered(listing_id, &gear("main", "456"), SELLER, ListingMode::Direct).await;
    f.lot("456", 1).await;
    sqlx::raw_sql(
        "CREATE FUNCTION public.fake_consignment_failure() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION '故意使测试事务失败'; END $$;
         CREATE CONSTRAINT TRIGGER fake_consignment_commit_failure
         AFTER INSERT ON public.consignment_events DEFERRABLE INITIALLY DEFERRED
         FOR EACH ROW EXECUTE FUNCTION public.fake_consignment_failure();",
    )
    .execute(f.owner.postgres_pool())
    .await
    .unwrap();
    let cmd_id = new_id();
    let closure = withdraw(cmd_id, listing_id, SELLER, 1);
    let before = f.persisted().await;
    assert_eq!(f.store.withdraw(closure).await, Err(ConsignmentError::Unavailable));
    assert!(f.persisted().await == before, "相关持久事实须保持一致");
    sqlx::raw_sql(
        "DROP TRIGGER fake_consignment_commit_failure ON public.consignment_events;
         CREATE TRIGGER fake_consignment_audit_failure BEFORE INSERT ON public.audit_records
         FOR EACH ROW EXECUTE FUNCTION public.fake_consignment_failure();",
    )
    .execute(f.owner.postgres_pool())
    .await
    .unwrap();
    assert_eq!(f.store.withdraw(closure).await, Err(ConsignmentError::Unavailable));
    assert!(f.persisted().await == before, "相关持久事实须保持一致");
    assert_eq!(f.command_count(cmd_id).await, 0);

    let unaffected_listing = new_id();
    let unaffected_gear = gear("main", "457");
    let fresh = || {
        super::fixture::register(
            new_id(),
            unaffected_listing,
            &unaffected_gear,
            &f.key,
            SELLER,
            ListingMode::Direct,
        )
    };
    assert_eq!(
        f.store.register(fresh(), Some(&f.crypto), Some(&f.crypto)).await,
        Err(ConsignmentError::Unavailable)
    );
    assert!(f.persisted().await == before, "相关持久事实须保持一致");
    sqlx::query("DROP TRIGGER fake_consignment_audit_failure ON public.audit_records")
        .execute(f.owner.postgres_pool())
        .await
        .unwrap();
    assert_eq!(
        f.store.withdraw(closure).await.unwrap().into_result(),
        Err(Rejection::ListingNotActive.into())
    );
    assert_eq!(f.command_count(cmd_id).await, 1);
    assert_eq!(
        f.state(listing_id).await,
        ("closed".into(), 2, Some("auction_lot_created".into()), false)
    );
    assert!(f.retired(listing_id).await);
}

#[tokio::test]
async fn postgres_auto_close_competes_with_withdraw_and_matches_frozen_deadline() {
    let f = Fixture::new().await;
    let listing_id = new_id();
    f.registered(listing_id, &gear("main", "458"), SELLER, ListingMode::Consigned).await;
    let frozen = expire_listing(&f, listing_id).await;
    let wrong = auto(new_id(), listing_id, AutoCloseEvidence::DisplayExpired(frozen));
    assert_eq!(
        f.store.auto_close(wrong).await.unwrap().into_result(),
        Err(ConsignmentError::ConditionNotMet)
    );
    let right = auto(new_id(), listing_id, AutoCloseEvidence::LookupDeadline(frozen));
    let cancel = withdraw(new_id(), listing_id, SELLER, 1);
    let blocker = f.block_control().await;
    let (a, b, ()) = tokio::join!(
        f.store.auto_close(right),
        f.store.withdraw(cancel),
        f.release_when_waiting(blocker, 2),
    );
    let effect = a.unwrap().into_result().unwrap();
    assert!(
        effect == ListingAutoCloseEffect::Unchanged
            || matches!(effect, ListingAutoCloseEffect::Terminated(projection) if projection.reason == Some(ListingReason::LookupDeadline))
    );
    let error = b.unwrap().into_result().unwrap_err();
    assert!(matches!(
        error,
        ConsignmentError::Business(Rejection::ListingNotActive | Rejection::StaleVersion)
    ));
    assert_eq!(
        f.state(listing_id).await,
        ("expired".into(), 2, Some("lookup_deadline".into()), false)
    );
    assert!(f.retired(listing_id).await);
}

#[tokio::test]
async fn postgres_lookup_cannot_publish_over_collision_or_disabled_epoch() {
    let f = Fixture::new().await;
    for (eid, reason) in
        [("459", ListingReason::AuctionLotCreated), ("460", ListingReason::ServiceDisabled)]
    {
        let listing_id = new_id();
        let equipment = gear("main", eid);
        let work_id = f.registered(listing_id, &equipment, SELLER, ListingMode::Direct).await;
        let fence = f.grant(listing_id, work_id, 1, 30).await;
        if reason == ListingReason::AuctionLotCreated {
            f.lot(eid, 1).await;
        } else {
            sqlx::query(
                "UPDATE public.consignment_lookup_control SET admitting=false, enabled_epoch=2",
            )
            .execute(f.pool())
            .await
            .unwrap();
        }
        let cmd = new_id();
        let input =
            || apply(cmd, fence, observation(&equipment, &f.key, "FakeWriteSeller", true), 1);
        let effect = f.store.apply_lookup(input()).await.unwrap().into_result().unwrap();
        assert!(matches!(effect, ListingLookupEffect::ListingChanged(projection) if
            projection.reason == Some(reason) && projection.state == ListingState::Closed));
        assert_eq!(
            f.state(listing_id).await,
            ("closed".into(), 2, Some(reason.as_str().into()), false)
        );
        assert!(f.retired(listing_id).await);
        assert_eq!(
            f.store.apply_lookup(input()).await.unwrap(),
            ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(effect))
        );
    }
}

#[tokio::test]
async fn postgres_auto_close_revalidates_stale_epoch_and_seller_history_evidence() {
    let f = Fixture::new().await;
    let seller_listing = new_id();
    let old_epoch_listing = new_id();
    f.registered(seller_listing, &gear("main", "469"), SELLER, ListingMode::Direct).await;
    f.registered(old_epoch_listing, &gear("isekai", "469"), OTHER, ListingMode::Consigned).await;
    let fact = f.set_blacklist(SELLER).await;
    sqlx::query("UPDATE public.users SET status='active' WHERE id=$1")
        .bind(SELLER)
        .execute(f.owner.postgres_pool())
        .await
        .unwrap();
    let unknown_id = new_id();
    let wrong_fact =
        auto(unknown_id, seller_listing, AutoCloseEvidence::SellerIneligible(new_id()));
    assert_eq!(
        f.store.auto_close(wrong_fact).await.unwrap().into_result(),
        Err(ConsignmentError::ConditionNotMet)
    );
    assert_eq!(f.command_count(unknown_id).await, 0);
    let blacklist = auto(new_id(), seller_listing, AutoCloseEvidence::SellerIneligible(fact));
    assert!(matches!(f.store.auto_close(blacklist).await.unwrap().into_result(),
        Ok(ListingAutoCloseEffect::Terminated(snapshot)) if snapshot.reason == Some(ListingReason::SellerIneligible)));
    assert!(f.retired(seller_listing).await);

    // 夹具直接推进单例到重开后的代次，仅验证 2D 写侧仍能识别上一代挂单。
    sqlx::query("UPDATE public.consignment_lookup_control SET admitting=true, enabled_epoch=2")
        .execute(f.pool())
        .await
        .unwrap();
    let wrong_epoch = auto(new_id(), old_epoch_listing, AutoCloseEvidence::ServiceDisabled(1));
    assert_eq!(
        f.store.auto_close(wrong_epoch).await.unwrap().into_result(),
        Err(ConsignmentError::ConditionNotMet)
    );
    let right_epoch = auto(new_id(), old_epoch_listing, AutoCloseEvidence::ServiceDisabled(2));
    assert!(matches!(f.store.auto_close(right_epoch).await.unwrap().into_result(),
        Ok(ListingAutoCloseEffect::Terminated(snapshot)) if snapshot.reason == Some(ListingReason::ServiceDisabled)));
    assert!(f.retired(old_epoch_listing).await);
    let current = new_id();
    f.registered(current, &gear("isekai", "469"), OTHER, ListingMode::Consigned).await;
    let current_epoch = auto(new_id(), current, AutoCloseEvidence::ServiceDisabled(2));
    assert_eq!(
        f.store.auto_close(current_epoch).await.unwrap().into_result(),
        Err(ConsignmentError::ConditionNotMet)
    );
    assert_eq!(f.state(current).await.1, 1);
}
