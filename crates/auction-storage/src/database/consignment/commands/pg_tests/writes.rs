//! 真实 PG 账本字段、重试、投影与审计回滚场景。

use std::sync::atomic::{AtomicUsize, Ordering};

use auction_application::consignment::{
    ConsignmentError, ListingBusinessRejection, ListingOperation, WrappedRegistrationReplayKey,
};
use auction_application::core::RequestHash;
use auction_domain::consignment::{
    ListingChange, ListingReason, ListingSnapshot, ListingState, ListingVersion,
};
use auction_observability::{AuditFailureKind, AuditSqlstateClass};
use sqlx::PgConnection;
use uuid::Uuid;

use super::diagnostics::capture_audit_failure;
use super::fixture::{
    COMMAND, KEY, LISTING, OTHER_SELLER, SELLER, client_command_id, counts, equipment, insert_key,
    later, price, register, registration_proof, reject, seed, start, time,
};
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::facts::{ChangedOperation, Decision};
use crate::database::consignment::commands::ledger::{
    Actor, Arbitration, RequestProof, StoredReceipt, Target,
};
use crate::database::consignment::commands::session::{LockKeys, with_command_transaction};
use crate::database::consignment::listings::projection::{
    lock_listings, pre_read_listing_identity,
};

fn command_keys(command_id: Uuid) -> LockKeys<'static> {
    LockKeys { command_id, sellers: &[], equipment: &[] }
}

fn next_version(version: u64) -> ListingVersion {
    ListingVersion::new(version).unwrap_or_else(|error| panic!("夹具版本：{error}"))
}

fn business<T>(result: Result<T, WriteError>) -> Option<ListingBusinessRejection> {
    match result {
        Err(WriteError::Application(ConsignmentError::Business(reason))) => Some(reason),
        _ => None,
    }
}

fn is_conflict<T>(result: Result<T, WriteError>) -> bool {
    matches!(result, Err(WriteError::Application(ConsignmentError::CommandIdConflict)))
}

fn assert_constraint<T>(result: Result<T, WriteError>, expected: &str) {
    assert!(
        matches!(result, Err(WriteError::Invariant { constraint, .. }) if constraint == expected)
    );
}

pub(super) async fn retry_policy(
    store: &DatabaseStore,
    maintenance: &DatabaseStore,
) -> Result<(), WriteError> {
    let mut observed = Vec::new();
    for (offset, sql, fail_until) in [
        (0, "DO $$ BEGIN RAISE serialization_failure; END $$", 2),
        (1, "DO $$ BEGIN RAISE deadlock_detected; END $$", 3),
        (
            2,
            "DO $$ BEGIN RAISE unique_violation USING CONSTRAINT = 'fake_unknown_unique'; END $$",
            3,
        ),
    ] {
        let command_id = client_command_id(1000 + offset);
        let attempts = AtomicUsize::new(0);
        let proof = RequestHash::new([55; 32]);
        let result =
            with_command_transaction(store, command_keys(command_id), &attempts, |attempts, tx| {
                Box::pin(async move {
                    let (replayed, receipt) = reject(tx, SELLER, LISTING, proof).await?;
                    assert!(!replayed, "上一尝试中的回执必须回滚");
                    if attempts.fetch_add(1, Ordering::Relaxed) < fail_until {
                        sqlx::query(sql).execute(&mut *tx.connection).await?;
                    }
                    Ok(receipt)
                })
            })
            .await;
        let result_is_expected = match offset {
            0 => result.is_ok_and(|receipt| receipt.body["command_id"] == command_id.to_string()),
            1 => {
                result.err().map(WriteError::into_application)
                    == Some(ConsignmentError::Unavailable)
            }
            _ => {
                matches!(result, Err(WriteError::Invariant { constraint, .. }) if constraint == "fake_unknown_unique")
            }
        };
        observed.push((attempts.load(Ordering::Relaxed), result_is_expected));
    }
    assert_eq!(observed, [(3, true), (3, true), (1, true)], "40001、40P01、23505");
    let saved: Vec<Vec<u8>> =
        sqlx::query_scalar("SELECT request_digest FROM public.consignment_commands")
            .fetch_all(maintenance.postgres_pool())
            .await?;
    assert_eq!(saved, vec![RequestHash::new([55; 32]).as_bytes().to_vec()]);
    Ok(())
}

pub(super) async fn compare_all_fields(
    store: &DatabaseStore,
    maintenance: &DatabaseStore,
) -> Result<(), WriteError> {
    seed(maintenance).await?;
    let original = register(store, COMMAND, LISTING, SELLER, "42").await?;
    let unexpected_mac =
        |_, _: &WrappedRegistrationReplayKey| panic!("账本身份不匹配时不能调用登记 MAC");
    let changed_mac = |key_id, wrapped: &WrappedRegistrationReplayKey| {
        assert_eq!(key_id, KEY);
        assert_eq!(wrapped.nonce(), &[0x22; 12]);
        Ok(registration_proof(KEY, 2).mac)
    };
    let same_actor_ref = SELLER.to_string();
    let mut unexpected_replays = Vec::new();
    for (field, request) in [
        (
            "actor_kind",
            (
                Actor::Admin(&same_actor_ref),
                ListingOperation::Register,
                Target::New,
                RequestProof::Registration(&unexpected_mac),
            ),
        ),
        (
            "actor_ref",
            (
                Actor::PublicUser(OTHER_SELLER),
                ListingOperation::Register,
                Target::New,
                RequestProof::Registration(&unexpected_mac),
            ),
        ),
        (
            "operation",
            (
                Actor::PublicUser(SELLER),
                ListingOperation::Withdraw,
                Target::New,
                RequestProof::Registration(&unexpected_mac),
            ),
        ),
        (
            "request_target",
            (
                Actor::PublicUser(SELLER),
                ListingOperation::Register,
                Target::Listing(LISTING),
                RequestProof::Registration(&unexpected_mac),
            ),
        ),
        (
            "mac",
            (
                Actor::PublicUser(SELLER),
                ListingOperation::Register,
                Target::New,
                RequestProof::Registration(&changed_mac),
            ),
        ),
        (
            "proof_kind",
            (
                Actor::PublicUser(SELLER),
                ListingOperation::Register,
                Target::New,
                RequestProof::Digest(RequestHash::new([1; 32])),
            ),
        ),
    ] {
        let result = with_command_transaction(
            store,
            command_keys(COMMAND),
            &request,
            |&(actor, operation, target, proof), tx| {
                Box::pin(async move {
                    match tx.arbitrate(actor, operation, target, proof).await? {
                        Arbitration::Replay(receipt) => Ok(receipt),
                        Arbitration::First(_) => panic!("已有命令不能取得首写能力"),
                    }
                })
            },
        )
        .await;
        if !is_conflict(result) {
            unexpected_replays.push(field);
        }
    }
    let repriced = manage(store, client_command_id(555), 125, false).await?;
    assert_eq!(repriced.body["price"], 125);
    assert_eq!(repriced.body["listing_version"], 2);
    assert_eq!(register(store, COMMAND, Uuid::from_u128(888), SELLER, "42").await?, original);
    assert_eq!(original.status, 202);
    assert_eq!(original.body["price"], 101);
    assert_eq!(original.body["listing_version"], 1);

    let repriced_id = client_command_id(555);
    let different_hash = RequestHash::new([126; 32]);
    if !is_conflict(
        with_command_transaction(store, command_keys(repriced_id), &(), |_, tx| {
            Box::pin(reject(tx, SELLER, LISTING, different_hash))
        })
        .await,
    ) {
        unexpected_replays.push("request_digest");
    }
    assert!(unexpected_replays.is_empty(), "这些字段改变后仍被重放：{unexpected_replays:?}");
    Ok(())
}

async fn manage(
    store: &DatabaseStore,
    command_id: Uuid,
    new_price: i64,
    forced: bool,
) -> Result<StoredReceipt, WriteError> {
    let identity = pre_read_listing_identity(store, LISTING)
        .await?
        .unwrap_or_else(|| panic!("测试应存在预读锁键"));
    with_command_transaction(
        store,
        LockKeys {
            command_id,
            sellers: &[identity.seller_user_id],
            equipment: &[identity.equipment],
        },
        &(),
        |_, tx| {
            Box::pin(async move {
                let proof = RequestProof::Digest(RequestHash::new(
                    [u8::try_from(new_price)
                        .unwrap_or_else(|error| panic!("夹具摘要标记：{error}"));
                        32],
                ));
                let Arbitration::First(mut writer) = tx
                    .arbitrate(
                        Actor::PublicUser(SELLER),
                        ListingOperation::Reprice,
                        Target::Listing(LISTING),
                        proof,
                    )
                    .await?
                else {
                    panic!("本测试命令应是首次");
                };
                let rows = lock_listings(writer.connection(), &[LISTING]).await?;
                let [current] = rows.as_slice() else {
                    panic!("测试应锁住一行");
                };
                if forced {
                    writer
                        .stage_system_closure(
                            current,
                            ListingSnapshot {
                                state: ListingState::Closed,
                                reason: Some(ListingReason::ServiceDisabled),
                                state_version: next_version(
                                    current.snapshot.state_version.value() + 1,
                                ),
                                ..current.snapshot
                            },
                            ListingChange::Closed,
                            later(),
                        )
                        .await?;
                    return writer
                        .finish(
                            Decision::Rejected {
                                listing_id: Some(LISTING),
                                reason: ListingBusinessRejection::ListingNotActive,
                            },
                            later(),
                        )
                        .await;
                }
                if current.snapshot.price == price(new_price) {
                    return writer.finish(Decision::PriceUnchanged(current), later()).await;
                }
                assert_eq!(
                    current.require_version(next_version(7)),
                    Err(ListingBusinessRejection::StaleVersion)
                );
                assert_eq!(current.require_version(ListingVersion::FIRST), Ok(()));
                writer
                    .stage_change(
                        current,
                        ListingSnapshot {
                            price: price(new_price),
                            state_version: next_version(2),
                            ..current.snapshot
                        },
                        current.current_snapshot_id,
                        ListingChange::Repriced,
                        later(),
                    )
                    .await?;
                writer
                    .finish(
                        Decision::Changed {
                            listing_id: LISTING,
                            operation: ChangedOperation::Reprice,
                        },
                        later(),
                    )
                    .await
            })
        },
    )
    .await
}

pub(super) async fn atomic_combinations(
    store: &DatabaseStore,
    maintenance: &DatabaseStore,
) -> Result<(), WriteError> {
    let pool = maintenance.postgres_pool();
    seed(maintenance).await?;
    assert_eq!(register(store, COMMAND, LISTING, SELLER, "42").await?.status, 202);
    assert_eq!(counts(pool).await?, (1, 1, 1, 1));

    let unchanged = manage(store, client_command_id(555), 101, false).await?;
    assert_eq!(unchanged.body["disposition"], "unchanged");
    assert_eq!(unchanged.body["listing_version"], 1);
    assert_eq!(counts(pool).await?, (1, 2, 1, 2));

    let closing_command = client_command_id(556);
    let refused = manage(store, closing_command, 125, true).await?;
    assert_eq!((refused.status, refused.code.as_str()), (409, "listing_not_active"));
    assert_eq!(counts(pool).await?, (1, 3, 2, 4));

    let facts: (String, i64, String, String, Uuid) = sqlx::query_as(
        "SELECT l.state, l.price, c.actor_kind, e.actor_kind, e.command_id
         FROM public.consignment_listings l
         JOIN public.consignment_events e ON e.listing_id = l.id AND e.state_version = l.state_version
         JOIN public.consignment_commands c USING (command_id)
         WHERE l.id = $1",
    ).bind(LISTING).fetch_one(pool).await?;
    assert_eq!(
        facts,
        ("closed".into(), 101, "public_user".into(), "system".into(), closing_command)
    );
    let audits: Vec<(String, String)> = sqlx::query_as(
        "SELECT actor_type, result FROM public.audit_records
         WHERE metadata->>'command_id' = $1 ORDER BY actor_type",
    )
    .bind(closing_command.to_string())
    .fetch_all(pool)
    .await?;
    assert_eq!(
        audits,
        vec![("public_user".into(), "rejected".into()), ("system".into(), "success".into())]
    );

    assert_eq!(
        register(store, client_command_id(557), Uuid::from_u128(558), SELLER, "42").await?.status,
        202
    );
    Ok(())
}

pub(super) async fn missing_fact_rejected(
    store: &DatabaseStore,
    maintenance: &DatabaseStore,
) -> Result<(), WriteError> {
    let pool = maintenance.postgres_pool();
    seed(maintenance).await?;
    assert_eq!(register(store, COMMAND, LISTING, SELLER, "42").await?.status, 202);
    // SQL 写成功才抵达断言；遗漏的事件/命令由提交阶段的延迟约束拒绝。
    let result = with_command_transaction(store, command_keys(Uuid::from_u128(12345)), &(), |_, tx| Box::pin(async move {
        sqlx::query("UPDATE public.consignment_listings SET price = 102, state_version = 2 WHERE id = $1")
            .bind(LISTING).execute(&mut *tx.connection).await?;
        Ok(())
    })).await;
    assert_constraint(result, "consignment_listing_version_event");

    let evidence = AtomicUsize::new(0);
    let result = with_command_transaction(store, command_keys(client_command_id(12346)), &evidence, |evidence, tx| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO public.consignment_commands
                   (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
                    listing_id, result_status, result_code, result_body, created_at)
                 VALUES ($1, 'public_user', $2, 'reprice', $3, decode(repeat('11',32),'hex'),
                         $4, 200, 'ok', $5, $6)",
            ).bind(tx.command_id).bind(SELLER.to_string()).bind(LISTING.to_string()).bind(LISTING)
              .bind(serde_json::json!({"command_id": tx.command_id, "operation": "reprice", "listing_id": LISTING,
                    "listing_version":2, "state":"resolving", "disposition":"repriced", "price":102}))
              .bind(start()).execute(&mut *tx.connection).await?;
            evidence.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })
    }).await;
    assert_eq!(evidence.load(Ordering::Relaxed), 1);
    assert_constraint(result, "consignment_command_version_event");
    assert_eq!(counts(pool).await?, (1, 1, 1, 1));
    Ok(())
}

/// 验证不变量 3、8：审计无法追加时，挂单、回执及事件都不能提交。
pub(super) async fn audit_failure_rolls_back(
    store: &DatabaseStore,
    maintenance: &DatabaseStore,
) -> Result<(), WriteError> {
    let pool = maintenance.postgres_pool();
    seed(maintenance).await?;
    sqlx::raw_sql(
        "CREATE FUNCTION public.consignment_fake_audit_failure() RETURNS trigger
         LANGUAGE plpgsql AS $$BEGIN RAISE check_violation USING CONSTRAINT = 'fake_audit_guard'; END;$$;
         CREATE TRIGGER consignment_fake_audit_failure BEFORE INSERT ON public.audit_records
         FOR EACH ROW WHEN (NEW.action = 'consignment.register') EXECUTE FUNCTION public.consignment_fake_audit_failure()",
    ).execute(pool).await?;
    let metrics = store.audit_metrics_source();
    let constraints = || {
        metrics
            .snapshot()
            .failures
            .into_iter()
            .find(|failure| {
                matches!(failure.kind, AuditFailureKind::ConstraintViolation)
                    && matches!(failure.sqlstate_class, AuditSqlstateClass::Constraint)
            })
            .map_or(0, |failure| failure.count)
    };
    let before = constraints();
    let result = capture_audit_failure(register(store, COMMAND, LISTING, SELLER, "42")).await;
    assert_constraint(result, "fake_audit_guard");
    assert!(constraints() > before, "审计约束失败计数应增加");
    assert!(metrics.snapshot().readiness_blocked);
    assert_eq!(counts(pool).await?, (0, 0, 0, 0));
    sqlx::query("DROP TRIGGER consignment_fake_audit_failure ON public.audit_records")
        .execute(pool)
        .await?;
    assert_eq!(register(store, COMMAND, LISTING, SELLER, "42").await?.status, 202);
    assert_eq!(counts(pool).await?, (1, 1, 1, 1));
    Ok(())
}

async fn publish(
    store: &DatabaseStore,
    seller: Uuid,
    listing_id: Uuid,
) -> Result<StoredReceipt, WriteError> {
    let command_id = Uuid::new_v5(&listing_id, b"fixture-lookup");
    let snapshot_id = Uuid::new_v4();
    let identity = equipment("42");
    with_command_transaction(
        store,
        LockKeys { command_id, sellers: &[seller], equipment: &[identity] },
        &(),
        |_, tx| Box::pin(async move {
            let Arbitration::First(mut writer) = tx.arbitrate(
                Actor::Worker, ListingOperation::ApplyLookup, Target::Listing(listing_id), RequestProof::Digest(RequestHash::new([3;32]))
            ).await? else { panic!("测试命令应是首次"); };
            let rows = lock_listings(writer.connection(), &[listing_id]).await?;
            let [current] = rows.as_slice() else { panic!("测试应锁住挂单"); };
            let conn: &mut PgConnection = writer.connection();
            sqlx::query(
                "INSERT INTO public.consignment_snapshots
                   (id, listing_id, generation, adapter, observed_at, digest, world_id,
                    external_equipment_id, name, category, level_kind, tradeable, owner_name, stat_groups)
                 VALUES ($1, $2, 1, 'hvdata-equip-v1', $3, decode(repeat('22',32),'hex'), 'main',
                         '42', 'Fake Equipment', 'Fake Category', 'unassigned', true, 'Fake Seller', '[]')",
            ).bind(snapshot_id).bind(listing_id).bind(later()).execute(conn).await?;
            writer.stage_change(
                current,
                ListingSnapshot {
                    state: ListingState::Displayed,
                    state_version: next_version(2),
                    expires_at: time("2026-01-08T00:00:00Z"),
                    ..current.snapshot
                },
                Some(snapshot_id),
                ListingChange::Published,
                later(),
            ).await?;
            writer.finish(Decision::Changed { listing_id, operation: ChangedOperation::ApplyLookup }, later()).await
        }),
    ).await
}

pub(super) async fn constraint_mapping(
    store: &DatabaseStore,
    maintenance: &DatabaseStore,
) -> Result<(), WriteError> {
    seed(maintenance).await?;
    let duplicate_id = Uuid::from_u128(402);
    register(store, COMMAND, LISTING, SELLER, "42").await?;
    let seller_conflict =
        business(register(store, client_command_id(302), duplicate_id, SELLER, "42").await);
    register(store, client_command_id(302), duplicate_id, OTHER_SELLER, "42").await?;
    assert_eq!(publish(store, SELLER, LISTING).await?.body["disposition"], "published");
    let equipment_conflict = business(publish(store, OTHER_SELLER, duplicate_id).await);
    assert_eq!(counts(maintenance.postgres_pool()).await?, (2, 3, 3, 3));

    let mut connection = store.postgres_pool().acquire().await?;
    let next_key = Uuid::from_u128(9999);
    let key_conflict = matches!(
        insert_key(&mut connection, next_key, true).await,
        Err(WriteError::ReplayKeyCurrent)
    );
    insert_key(&mut connection, next_key, false).await?;
    assert_eq!(
        (seller_conflict, equipment_conflict, key_conflict),
        (
            Some(ListingBusinessRejection::AlreadyOpen),
            Some(ListingBusinessRejection::EquipmentBusy),
            true
        )
    );
    Ok(())
}
