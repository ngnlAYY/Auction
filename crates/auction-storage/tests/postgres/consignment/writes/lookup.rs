//! 解析观察分支、活围栏及全局冷却的数据库行为。

use std::time::Duration;

use auction_application::consignment::{
    ConsignmentError, ListingBusinessRejection, ListingCommandOutcome, ListingCommandRejection,
    ListingCommandStore, ListingEquipmentLookupError, ListingLookupEffect, LookupFence,
    LookupObservation, LookupPermanentReason, LookupRetryAfter, LookupTransientReason,
    StoredListingDecision,
};
use auction_domain::consignment::{
    ListingMode, ListingPublicationTtls, ListingReason, ListingState,
};
use auction_time::{Duration as TimeDelta, Timestamp};
use uuid::Uuid;

use super::fixture::{
    Fixture, OTHER, SELLER, apply, gear, new_id, new_internal_command_id, observation, reprice,
    time_delta, withdraw,
};

#[tokio::test]
async fn postgres_apply_lookup_replays_after_reprice_withdraw_and_lease_changes() {
    let f = Fixture::new().await;
    let g = gear("main", "441");
    let listing_id = new_id();
    let work_id = f.registered(listing_id, &g, SELLER, ListingMode::Direct).await;
    let fence = f.grant(listing_id, work_id, 1, 30).await;
    let initial_deadline = f.deadline(listing_id).await;
    f.store
        .reprice(reprice(new_id(), listing_id, SELLER, 1, 765))
        .await
        .unwrap()
        .into_result()
        .unwrap();
    let cmd_id = new_internal_command_id();
    let cmd =
        |variant| apply(cmd_id, fence, observation(&g, &f.key, "fakewriteseller", true), variant);
    let published = f.store.apply_lookup(cmd(1)).await.unwrap().into_result().unwrap();
    let ListingLookupEffect::ListingChanged(published) = published else { panic!("期望发布") };
    assert_eq!(published.state, ListingState::Displayed);
    assert_eq!(published.price.value(), 765);
    assert_eq!(published.state_version.value(), 3);
    assert!(published.expires_at > initial_deadline);
    assert!(f.retired(listing_id).await);
    assert!(f.state(listing_id).await.3);
    f.store
        .withdraw(withdraw(new_id(), listing_id, SELLER, 3))
        .await
        .unwrap()
        .into_result()
        .unwrap();
    assert!(f.state(listing_id).await.3, "已发布的安全快照须在关闭后保留");
    assert_eq!(
        f.store.apply_lookup(cmd(1)).await.unwrap(),
        ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(
            ListingLookupEffect::ListingChanged(published)
        ))
    );
    assert_eq!(
        f.store.apply_lookup(cmd(2)).await.unwrap().into_result(),
        Err(ConsignmentError::CommandIdConflict)
    );
}

#[tokio::test]
async fn postgres_apply_lookup_permanent_and_private_safe_snapshot_branches() {
    let f = Fixture::new().await;
    let permanent = [
        (
            ListingEquipmentLookupError::Permanent(LookupPermanentReason::EquipmentNotFound),
            "equipment_not_found",
        ),
        (
            ListingEquipmentLookupError::Permanent(LookupPermanentReason::SourceRequestRejected),
            "source_request_rejected",
        ),
        (
            ListingEquipmentLookupError::Permanent(LookupPermanentReason::SourceHttpInvalid),
            "source_http_invalid",
        ),
        (
            ListingEquipmentLookupError::Permanent(LookupPermanentReason::SourceSchemaInvalid),
            "source_schema_invalid",
        ),
        (
            ListingEquipmentLookupError::Permanent(LookupPermanentReason::SourceTooLarge),
            "source_too_large",
        ),
        (
            ListingEquipmentLookupError::Permanent(LookupPermanentReason::SourceShapeUnconfirmed),
            "source_shape_unconfirmed",
        ),
        (
            ListingEquipmentLookupError::Permanent(LookupPermanentReason::SourceIdentityMismatch),
            "source_identity_mismatch",
        ),
        (
            ListingEquipmentLookupError::Permanent(LookupPermanentReason::OwnerMissing),
            "owner_missing",
        ),
        (ListingEquipmentLookupError::CredentialUnavailable, "credential_unavailable"),
    ];
    for (n, (error, reason)) in permanent.into_iter().enumerate() {
        let g = gear("main", &(510 + n).to_string());
        let listing_id = new_id();
        let work_id = f.registered(listing_id, &g, SELLER, ListingMode::Direct).await;
        let fence = f.grant(listing_id, work_id, 1, 30).await;
        f.store
            .apply_lookup(apply(
                new_internal_command_id(),
                fence,
                LookupObservation::Failed(error),
                1,
            ))
            .await
            .unwrap()
            .into_result()
            .unwrap();
        assert_eq!(f.state(listing_id).await, ("rejected".into(), 2, Some(reason.into()), false));
        assert!(f.retired(listing_id).await);
        let done: (String, Option<Timestamp>) = sqlx::query_as(
            "SELECT state,next_attempt_at FROM public.consignment_lookup_work WHERE id=$1",
        )
        .bind(work_id)
        .fetch_one(f.pool())
        .await
        .unwrap();
        assert_eq!(done, ("done".into(), None));
    }
    for (n, (mode, owner, tradeable, expected)) in [
        (ListingMode::Direct, "FakeWriteOther", false, "not_tradeable"),
        (ListingMode::Consigned, "FakeWriteOther", false, "not_tradeable"),
        (ListingMode::Direct, "FakeWriteOther", true, "owner_mismatch"),
        (ListingMode::Consigned, "FakeWriteSeller", true, ""),
    ]
    .into_iter()
    .enumerate()
    {
        let g = gear("isekai", &(610 + n).to_string());
        let listing_id = new_id();
        let work_id = f.registered(listing_id, &g, OTHER, mode).await;
        let fence = f.grant(listing_id, work_id, 1, 30).await;
        // 逐行确认私有拒绝优先序，最后一个用真正相符的名字发布为待寄。
        let actual_owner = if expected == "owner_mismatch" {
            "FakeWriteSeller"
        } else if expected.is_empty() {
            "FakeWriteOther"
        } else {
            owner
        };
        f.store
            .apply_lookup(apply(
                new_internal_command_id(),
                fence,
                observation(&g, &f.key, actual_owner, tradeable),
                1,
            ))
            .await
            .unwrap()
            .into_result()
            .unwrap();
        if expected.is_empty() {
            assert_eq!(f.state(listing_id).await, ("awaiting_deposit".into(), 2, None, true));
        } else {
            assert_eq!(
                f.state(listing_id).await,
                ("rejected".into(), 2, Some(expected.into()), true)
            );
        }
        assert!(f.retired(listing_id).await);
    }
}

#[tokio::test]
async fn postgres_apply_lookup_first_qualified_publication_wins_equipment() {
    let f = Fixture::new().await;
    let g = gear("main", "442");
    let first = new_id();
    let second = new_id();
    let first_work = f.registered(first, &g, SELLER, ListingMode::Direct).await;
    let second_work = f.registered(second, &g, OTHER, ListingMode::Consigned).await;
    assert_eq!(f.state(first).await.0, "resolving");
    assert_eq!(f.state(second).await.0, "resolving");
    // 全局只能有一条活许可。让第一条提交的发布和第二条原始观察跨提交边界竞争。
    let f1 = f.grant(first, first_work, 1, 30).await;
    let f2 = LookupFence::new(second_work, second, 1, 1, new_id()).unwrap();
    let command = new_internal_command_id();
    let blocker = f.block_control().await;
    let (a, b, ()) = tokio::join!(
        f.store.apply_lookup(apply(
            new_internal_command_id(),
            f1,
            observation(&g, &f.key, "FakeWriteSeller", true),
            1
        )),
        f.store.apply_lookup(apply(
            command,
            f2,
            observation(&g, &f.key, "FakeWriteOther", true),
            1
        )),
        f.release_when_waiting(blocker, 2),
    );
    assert!(a.unwrap().into_result().is_ok());
    assert_eq!(b.unwrap().into_result(), Err(ConsignmentError::LeaseLost));
    assert_eq!(f.command_count(command).await, 0);
    let second_fence = f.grant(second, second_work, 1, 30).await;
    f.store
        .apply_lookup(apply(
            command,
            second_fence,
            observation(&g, &f.key, "FakeWriteOther", true),
            1,
        ))
        .await
        .unwrap()
        .into_result()
        .unwrap();
    assert_eq!(f.state(first).await, ("displayed".into(), 2, None, true));
    assert_eq!(f.state(second).await, ("rejected".into(), 2, Some("equipment_busy".into()), true));
    assert!(f.retired(second).await);
}

#[tokio::test]
async fn postgres_apply_lookup_lost_fence_leaves_all_rows_and_next_permit_untouched() {
    let f = Fixture::new().await;
    let g = gear("main", "443");
    let listing_id = new_id();
    let work_id = f.registered(listing_id, &g, SELLER, ListingMode::Direct).await;
    let good = f.grant(listing_id, work_id, 1, 30).await;
    let cmd_id = new_internal_command_id();
    let observe = || {
        LookupObservation::Failed(ListingEquipmentLookupError::SourceRateLimited(Some(
            LookupRetryAfter::new(Duration::from_secs(3600)),
        )))
    };
    let before = f.persisted().await;
    for fence in [
        LookupFence::new(work_id, listing_id, 2, good.fence(), good.token()).unwrap(),
        LookupFence::new(work_id, listing_id, 1, good.fence() + 1, good.token()).unwrap(),
        LookupFence::new(work_id, listing_id, 1, good.fence(), new_id()).unwrap(),
        LookupFence::new(work_id, new_id(), 1, good.fence(), good.token()).unwrap(),
    ] {
        assert_eq!(
            f.store.apply_lookup(apply(cmd_id, fence, observe(), 1)).await.unwrap().into_result(),
            Err(ConsignmentError::LeaseLost)
        );
        assert!(f.persisted().await == before, "相关持久事实须保持一致");
    }
    let expired = f.grant(listing_id, work_id, 2, -1).await;
    let before = f.persisted().await;
    assert_eq!(
        f.store.apply_lookup(apply(cmd_id, expired, observe(), 1)).await.unwrap().into_result(),
        Err(ConsignmentError::LeaseLost)
    );
    assert!(f.persisted().await == before, "相关持久事实须保持一致");
    let late = f.grant(listing_id, work_id, 3, 30).await;
    f.store
        .withdraw(withdraw(new_id(), listing_id, SELLER, 1))
        .await
        .unwrap()
        .into_result()
        .unwrap();
    let old = f.persisted().await;
    assert_eq!(
        f.store.apply_lookup(apply(cmd_id, late, observe(), 1)).await.unwrap().into_result(),
        Err(ConsignmentError::LeaseLost)
    );
    assert!(f.persisted().await == old, "相关持久事实须保持一致");
    let new_listing = new_id();
    let new_work =
        f.registered(new_listing, &gear("main", "444"), SELLER, ListingMode::Direct).await;
    f.grant(new_listing, new_work, 1, 30).await;
    let new = f.persisted().await;
    assert_eq!(
        f.store.apply_lookup(apply(cmd_id, late, observe(), 1)).await.unwrap().into_result(),
        Err(ConsignmentError::LeaseLost)
    );
    assert!(f.persisted().await == new, "相关持久事实须保持一致");
    assert_eq!(f.command_count(cmd_id).await, 0);
}

#[tokio::test]
async fn postgres_apply_lookup_retries_six_times_and_caps_429_cooldown() {
    let f = Fixture::new().await;
    let listing_id = new_id();
    let work_id = f.registered(listing_id, &gear("main", "445"), SELLER, ListingMode::Direct).await;
    let initial_deadline = f.deadline(listing_id).await;
    for (idx, seconds) in [30, 120, 600, 1800, 7200, 0].into_iter().enumerate() {
        let attempt = i16::try_from(idx + 1).unwrap();
        let fence = f.grant(listing_id, work_id, attempt, 30).await;
        let error = match attempt {
            1 => ListingEquipmentLookupError::SourceUnavailable,
            2 | 4 | 5 => ListingEquipmentLookupError::SourceTimeout,
            3 => ListingEquipmentLookupError::SourceRateLimited(None),
            6 => ListingEquipmentLookupError::SourceRateLimited(Some(LookupRetryAfter::new(
                Duration::from_secs(40_000),
            ))),
            _ => panic!("固定夹具计次"),
        };
        let cmd_id = new_internal_command_id();
        let cmd = || apply(cmd_id, fence, LookupObservation::Failed(error), 1);
        let effect = f.store.apply_lookup(cmd()).await.unwrap().into_result().unwrap();
        let (state, last, next, at, holder, cooldown): (String, String, Option<Timestamp>, Timestamp, Option<Uuid>, Option<Timestamp>) =
            sqlx::query_as(
                "SELECT w.state,w.last_reason,w.next_attempt_at,w.updated_at,c.holder_work_id,c.cooldown_until
                 FROM public.consignment_lookup_work w CROSS JOIN public.consignment_lookup_control c WHERE w.id=$1",
            ).bind(work_id).fetch_one(f.pool()).await.unwrap();
        let reason = error.transient_reason().unwrap();
        assert_eq!(last, reason.as_str());
        assert!(holder.is_none());
        assert_eq!(f.deadline(listing_id).await, initial_deadline);
        if attempt < 6 {
            assert_eq!(state, "queued");
            assert_eq!(next, Some(at + time_delta(seconds)));
            assert_eq!(
                effect,
                ListingLookupEffect::Rescheduled {
                    next_attempt_at: at + time_delta(seconds),
                    reason
                }
            );
            assert_eq!(f.state(listing_id).await, ("resolving".into(), 1, None, false));
            assert!(!f.retired(listing_id).await);
        } else {
            assert_eq!(state, "done");
            assert!(next.is_none());
            assert!(
                matches!(effect, ListingLookupEffect::ListingChanged(projection) if projection.reason == Some(ListingReason::LookupExhausted))
            );
            assert!(f.retired(listing_id).await);
        }
        if reason == LookupTransientReason::SourceRateLimited {
            assert_eq!(cooldown, Some(at + time_delta(if attempt == 6 { 21600 } else { 600 })));
        }
        assert_eq!(
            f.store.apply_lookup(cmd()).await.unwrap(),
            ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(effect))
        );
    }
}

#[tokio::test]
async fn postgres_apply_lookup_final_cas_rolls_back_snapshot_when_lease_expires_mid_write() {
    let f = Fixture::new().await;
    let g = gear("main", "446");
    let listing_id = new_id();
    let work_id = f.registered(listing_id, &g, SELLER, ListingMode::Direct).await;
    // 只有实际走到写快照才等待数据库内已知的租约截止，用数据库时钟触发最终 CAS。
    sqlx::raw_sql(
        "CREATE FUNCTION public.fake_consignment_expire_lease() RETURNS trigger
           LANGUAGE plpgsql AS $$
         BEGIN
           PERFORM pg_sleep_until(w.lease_until) FROM public.consignment_lookup_work w
             WHERE w.listing_id=NEW.listing_id AND w.generation=NEW.generation;
           RETURN NEW;
         END $$;
         CREATE TRIGGER fake_consignment_expire_lease BEFORE INSERT ON public.consignment_snapshots
           FOR EACH ROW EXECUTE FUNCTION public.fake_consignment_expire_lease();",
    )
    .execute(f.owner.postgres_pool())
    .await
    .unwrap();
    let fence = f.grant(listing_id, work_id, 1, 2).await;
    let original = f.persisted().await;
    let cmd_id = new_internal_command_id();
    let result = f
        .store
        .apply_lookup(apply(cmd_id, fence, observation(&g, &f.key, "FakeWriteSeller", true), 1))
        .await
        .unwrap()
        .into_result();
    assert_eq!(result, Err(ConsignmentError::LeaseLost));
    assert!(f.persisted().await == original, "相关持久事实须保持一致");
    assert_eq!(f.command_count(cmd_id).await, 0);

    sqlx::query("DROP TRIGGER fake_consignment_expire_lease ON public.consignment_snapshots")
        .execute(f.owner.postgres_pool())
        .await
        .unwrap();
    let fresh = f.grant(listing_id, work_id, 2, 30).await;
    let applied = f
        .store
        .apply_lookup(apply(
            new_internal_command_id(),
            fresh,
            observation(&g, &f.key, "FakeWriteSeller", true),
            1,
        ))
        .await
        .unwrap()
        .into_result()
        .unwrap();
    assert!(
        matches!(applied, ListingLookupEffect::ListingChanged(snapshot) if snapshot.state == ListingState::Displayed)
    );
}

#[tokio::test]
async fn postgres_apply_lookup_uses_publication_time_and_mode_specific_configured_ttl() {
    let f = Fixture::new().await;
    let pool = f.owner.postgres_pool();
    for (eid, mode, state, days) in [
        ("447", ListingMode::Direct, ListingState::Displayed, 37),
        ("448", ListingMode::Consigned, ListingState::AwaitingDeposit, 9),
    ] {
        let g = gear("main", eid);
        let listing_id = new_id();
        let work_id = f.registered(listing_id, &g, SELLER, mode).await;
        let fence = f.grant(listing_id, work_id, 1, 30).await;
        let mut cmd = apply(
            new_internal_command_id(),
            fence,
            observation(&g, &f.key, "FakeWriteSeller", true),
            1,
        );
        cmd.publication_ttls = ListingPublicationTtls::new(37, 9).unwrap();
        let published = f.store.apply_lookup(cmd).await.unwrap().into_result().unwrap();
        let (published_at, expires_at, event_at): (Timestamp, Timestamp, Timestamp) = sqlx::query_as(
            "SELECT l.updated_at,l.expires_at,e.occurred_at FROM public.consignment_listings l
             JOIN public.consignment_events e ON e.listing_id=l.id AND e.state_version=l.state_version
             WHERE l.id=$1 AND e.event_kind='published'",
        ).bind(listing_id).fetch_one(pool).await.unwrap();
        assert_eq!(event_at, published_at);
        assert_eq!(expires_at, published_at + TimeDelta::days(days), "{mode:?}");
        assert!(matches!(published, ListingLookupEffect::ListingChanged(listing)
            if listing.state == state && listing.expires_at == expires_at));
    }
}

#[tokio::test]
async fn postgres_apply_lookup_cannot_publish_when_snapshot_write_crosses_listing_deadline() {
    let f = Fixture::new().await;
    let g = gear("main", "449");
    let listing_id = new_id();
    let work_id = f.registered(listing_id, &g, SELLER, ListingMode::Direct).await;
    let pool = f.owner.postgres_pool();
    let fence = f.grant(listing_id, work_id, 1, 30).await;
    let mut tx = pool.begin().await.unwrap();
    sqlx::raw_sql(
        "CREATE FUNCTION public.fake_consignment_wait_for_listing_deadline() RETURNS trigger
           LANGUAGE plpgsql AS $$
         BEGIN
           PERFORM pg_sleep_until(l.expires_at) FROM public.consignment_listings l WHERE l.id=NEW.listing_id;
           RETURN NEW;
         END $$;
         CREATE TRIGGER fake_consignment_wait_for_listing_deadline
           BEFORE INSERT ON public.consignment_snapshots
           FOR EACH ROW EXECUTE FUNCTION public.fake_consignment_wait_for_listing_deadline();
         ALTER TABLE public.consignment_listings DISABLE TRIGGER USER;",
    ).execute(&mut *tx).await.unwrap();
    let cutoff: Timestamp = sqlx::query_scalar(
        "UPDATE public.consignment_listings SET expires_at=statement_timestamp() + interval '2 seconds'
         WHERE id=$1 RETURNING expires_at",
    ).bind(listing_id).fetch_one(&mut *tx).await.unwrap();
    sqlx::query("ALTER TABLE public.consignment_listings ENABLE TRIGGER USER")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let cmd_id = new_internal_command_id();
    let received = f
        .store
        .apply_lookup(apply(cmd_id, fence, observation(&g, &f.key, "FakeWriteSeller", true), 1))
        .await
        .unwrap();
    assert_eq!(
        received,
        ListingCommandOutcome::Rejected(ListingCommandRejection::Recorded(
            ListingBusinessRejection::ListingNotActive
        ))
    );
    assert_eq!(
        f.state(listing_id).await,
        ("expired".into(), 2, Some("lookup_deadline".into()), false)
    );
    assert_eq!(f.deadline(listing_id).await, cutoff);
    let (written_at, completed_at): (Timestamp, Timestamp) = sqlx::query_as(
        "SELECT l.updated_at,w.completed_at FROM public.consignment_listings l
         JOIN public.consignment_lookup_work w ON w.listing_id=l.id WHERE l.id=$1 AND w.state='done'"
    ).bind(listing_id).fetch_one(pool).await.unwrap();
    assert!(written_at >= cutoff && completed_at >= cutoff);
    assert!(f.retired(listing_id).await);
}
