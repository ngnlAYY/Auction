//! 逐操作重放、Admin 私有说明与登记比较钥轮换。

use auction_application::consignment::{
    ConsignmentError, ListingDelistReason, ListingOperation, LookupTransientReason,
    WrappedRegistrationReplayKey,
};
use auction_application::core::RequestHash;
use auction_domain::consignment::{
    ListingChange, ListingReason, ListingSnapshot, ListingState, ListingVersion,
};
use uuid::Uuid;

use super::fixture::{
    COMMAND, KEY, LISTING, SELLER, counts, equipment, insert_key, later, register,
    registration_proof, seed, time,
};
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::facts::{ChangedOperation, Decision};
use crate::database::consignment::commands::ledger::{
    Actor, Arbitration, RequestProof, StoredReceipt, Target,
};
use crate::database::consignment::commands::session::{LockKeys, with_command_transaction};
use crate::database::consignment::listings::projection::lock_listings;

const OPERATION_COMMAND: Uuid = Uuid::from_u128(555);
const ADMIN_REASON: &str = "测试私有下架说明";

pub(super) async fn registration_replays_with_old_key(
    store: &DatabaseStore,
    maintenance: &DatabaseStore,
) -> Result<(), WriteError> {
    seed(maintenance).await?;
    let original = register(store, COMMAND, LISTING, SELLER, "42").await?;
    let replacement = Uuid::from_u128(202);
    let mut rotation = maintenance.postgres_pool().begin().await?;
    sqlx::query("UPDATE public.consignment_replay_keys SET write_current = false WHERE id = $1")
        .bind(KEY)
        .execute(&mut *rotation)
        .await?;
    insert_key(&mut rotation, replacement, true).await?;
    rotation.commit().await?;

    let compute_mac = |key_id, wrapped: &WrappedRegistrationReplayKey| {
        assert_eq!(key_id, KEY, "重放应取账本绑定的旧逻辑钥");
        assert_eq!(wrapped.nonce(), &[0x22; 12], "必须将旧逻辑钥的包装项交给调用方");
        Ok(registration_proof(KEY, 1).mac)
    };
    let replay = with_command_transaction(
        store,
        LockKeys { command_id: COMMAND, sellers: &[], equipment: &[] },
        &compute_mac,
        |compute_mac, tx| {
            Box::pin(async move {
                match tx
                    .arbitrate(
                        Actor::PublicUser(SELLER),
                        ListingOperation::Register,
                        Target::New,
                        RequestProof::Registration(compute_mac),
                    )
                    .await?
                {
                    Arbitration::Replay(receipt) => Ok(receipt),
                    Arbitration::First(_) => panic!("轮换后已有登记应当重放"),
                }
            })
        },
    )
    .await?;
    assert_eq!(replay, original);
    assert_eq!(counts(maintenance.postgres_pool()).await?, (1, 1, 1, 1));
    Ok(())
}

#[derive(Clone, Copy)]
enum Operation<'a> {
    Withdraw(ListingVersion),
    AdminDelist(&'a ListingDelistReason),
    AdminDelistWithoutReason,
    Reschedule(LookupTransientReason),
    AutoClose(ListingReason),
}

impl Operation<'_> {
    fn header(self) -> (Actor<'static>, ListingOperation) {
        match self {
            Self::Withdraw(_) => (Actor::PublicUser(SELLER), ListingOperation::Withdraw),
            Self::AdminDelist(_) | Self::AdminDelistWithoutReason => {
                (Actor::Admin("fake_admin"), ListingOperation::AdminDelist)
            }
            Self::Reschedule(_) => (Actor::Worker, ListingOperation::ApplyLookup),
            Self::AutoClose(_) => (Actor::System, ListingOperation::AutoClose),
        }
    }
}

async fn execute(
    store: &DatabaseStore,
    operation: Operation<'_>,
    request_hash: RequestHash,
) -> Result<StoredReceipt, WriteError> {
    let keys = LockKeys {
        command_id: OPERATION_COMMAND,
        sellers: &[SELLER],
        equipment: &[equipment("42")],
    };
    with_command_transaction(store, keys, &operation, move |&operation, tx| {
        Box::pin(async move {
            let (actor, listing_operation) = operation.header();
            let mut writer = match tx
                .arbitrate(
                    actor,
                    listing_operation,
                    Target::Listing(LISTING),
                    RequestProof::Digest(request_hash),
                )
                .await?
            {
                Arbitration::Replay(receipt) => return Ok(receipt),
                Arbitration::First(writer) => writer,
            };
            let rows = lock_listings(writer.connection(), &[LISTING]).await?;
            let [current] = rows.as_slice() else { panic!("应锁住挂单") };
            if let Operation::Reschedule(reason) = operation {
                return writer
                    .finish(
                        Decision::LookupRescheduled {
                            listing_id: LISTING,
                            next_attempt_at: time("2026-01-01T01:15:00Z"),
                            reason,
                        },
                        later(),
                    )
                    .await;
            }
            let (reason, changed) = match operation {
                Operation::Withdraw(expected_version) => {
                    current.require_version(expected_version).map_err(ConsignmentError::from)?;
                    (ListingReason::SellerCancelled, ChangedOperation::Withdraw)
                }
                Operation::AdminDelist(reason) => {
                    (ListingReason::AdminDelisted, ChangedOperation::AdminDelist(reason))
                }
                // 保留原故障的尝试：在 Admin 仲裁与投影变动之后交出无说明的成功处置。
                Operation::AdminDelistWithoutReason => {
                    (ListingReason::AdminDelisted, ChangedOperation::Withdraw)
                }
                Operation::AutoClose(reason) => (reason, ChangedOperation::AutoClose),
                Operation::Reschedule(_) => panic!("重排无挂单版本变化"),
            };
            let projection = ListingSnapshot {
                state: ListingState::Closed,
                reason: Some(reason),
                state_version: ListingVersion::new(2)
                    .unwrap_or_else(|error| panic!("夹具版本：{error}")),
                ..current.snapshot
            };
            if matches!(operation, Operation::AutoClose(_)) {
                writer
                    .stage_system_closure(current, projection, ListingChange::Closed, later())
                    .await?;
            } else {
                writer
                    .stage_change(
                        current,
                        projection,
                        current.current_snapshot_id,
                        ListingChange::Closed,
                        later(),
                    )
                    .await?;
            }
            writer
                .finish(Decision::Changed { listing_id: LISTING, operation: changed }, later())
                .await
        })
    })
    .await
}

async fn verify_replay(
    store: &DatabaseStore,
    operation: Operation<'_>,
    changed: Operation<'_>,
    original: StoredReceipt,
) -> Result<StoredReceipt, WriteError> {
    // 两个上游摘要只代表一次语义字段变化，编码由应用层测试。
    assert_eq!(execute(store, operation, RequestHash::new([7; 32])).await?, original);
    assert!(
        matches!(
            execute(store, changed, RequestHash::new([8; 32])).await,
            Err(WriteError::Application(ConsignmentError::CommandIdConflict))
        ),
        "变更后的上游摘要不能得到原回执"
    );
    assert_eq!(original.body["command_id"], OPERATION_COMMAND.to_string());
    assert_eq!(original.body["operation"], operation.header().1.as_str());
    assert_eq!((original.status, original.code.as_str()), (200, "ok"));
    Ok(original)
}

async fn seed_listing(
    store: &DatabaseStore,
    maintenance: &DatabaseStore,
) -> Result<(), WriteError> {
    seed(maintenance).await?;
    register(store, COMMAND, LISTING, SELLER, "42").await?;
    Ok(())
}

pub(super) async fn withdraw_replay(
    store: &DatabaseStore,
    maintenance: &DatabaseStore,
) -> Result<(), WriteError> {
    seed_listing(store, maintenance).await?;
    let operation = Operation::Withdraw(ListingVersion::FIRST);
    let changed = Operation::Withdraw(
        ListingVersion::new(2).unwrap_or_else(|error| panic!("夹具预期版本：{error}")),
    );
    let first = execute(store, operation, RequestHash::new([7; 32])).await?;
    let receipt = verify_replay(store, operation, changed, first).await?;
    assert_eq!(receipt.body["disposition"], "closed");
    assert_eq!(receipt.body["listing_version"], 2);
    assert_eq!(counts(maintenance.postgres_pool()).await?, (1, 2, 2, 2));
    Ok(())
}

pub(super) async fn admin_delist_requires_private_reason(
    store: &DatabaseStore,
    maintenance: &DatabaseStore,
) -> Result<(), WriteError> {
    seed_listing(store, maintenance).await?;
    let pool = maintenance.postgres_pool();
    let digest = RequestHash::new([7; 32]);
    let missing = execute(store, Operation::AdminDelistWithoutReason, digest).await;
    assert!(
        matches!(missing, Err(WriteError::Invariant { constraint, .. })
        if constraint == "consignment_command_decision_operation"),
        "缺说明的成功处置必须失败"
    );
    assert_eq!(counts(pool).await?, (1, 1, 1, 1));
    let (state, version, reason): (String, i64, Option<String>) = sqlx::query_as(
        "SELECT state, state_version, reason_code FROM public.consignment_listings WHERE id = $1",
    )
    .bind(LISTING)
    .fetch_one(pool)
    .await?;
    assert_eq!((state.as_str(), version, reason), ("resolving", 1, None));

    let reason = ListingDelistReason::new(ADMIN_REASON.into())?;
    let receipt = execute(store, Operation::AdminDelist(&reason), digest).await?;
    assert_eq!(receipt.body["disposition"], "closed");
    let (audit_reason, operation, event_reason, receipt_contains, event_contains, listing_contains):
        (String, String, String, bool, bool, bool) = sqlx::query_as(
        "SELECT a.metadata->>'admin_delist_reason', c.operation, e.reason_code,
                strpos(c.result_body::text, $2) > 0, strpos(row_to_json(e)::text, $2) > 0,
                strpos(row_to_json(l)::text, $2) > 0
           FROM public.audit_records a JOIN public.consignment_commands c ON
                a.metadata->>'command_id' = c.command_id::text
           JOIN public.consignment_events e USING (command_id) JOIN public.consignment_listings l
                ON l.id = e.listing_id
          WHERE c.command_id = $1 AND a.actor_type = 'admin' AND a.result = 'success'"
    ).bind(OPERATION_COMMAND).bind(ADMIN_REASON).fetch_one(pool).await?;
    assert_eq!(audit_reason, ADMIN_REASON);
    assert_eq!(operation, "admin_delist");
    assert_eq!(event_reason, "admin_delisted");
    assert!(!receipt_contains && !event_contains && !listing_contains);
    assert!(!receipt.body.to_string().contains(ADMIN_REASON));
    assert_eq!(counts(pool).await?, (1, 2, 2, 2));
    Ok(())
}

pub(super) async fn admin_delist_replay(
    store: &DatabaseStore,
    maintenance: &DatabaseStore,
) -> Result<(), WriteError> {
    seed_listing(store, maintenance).await?;
    let reason = ListingDelistReason::new(ADMIN_REASON.into())?;
    let operation = Operation::AdminDelist(&reason);
    let first = execute(store, operation, RequestHash::new([7; 32])).await?;
    let changed_reason = ListingDelistReason::new("另一条测试私有说明".into())?;
    let receipt =
        verify_replay(store, operation, Operation::AdminDelist(&changed_reason), first).await?;
    assert_eq!(receipt.body["disposition"], "closed");
    assert_eq!(receipt.body["listing_version"], 2);
    assert_eq!(counts(maintenance.postgres_pool()).await?, (1, 2, 2, 2));
    Ok(())
}

pub(super) async fn lookup_rescheduled_replay(
    store: &DatabaseStore,
    maintenance: &DatabaseStore,
) -> Result<(), WriteError> {
    seed_listing(store, maintenance).await?;
    let operation = Operation::Reschedule(LookupTransientReason::SourceTimeout);
    let changed = Operation::Reschedule(LookupTransientReason::SourceUnavailable);
    let first = execute(store, operation, RequestHash::new([7; 32])).await?;
    let receipt = verify_replay(store, operation, changed, first).await?;
    assert_eq!(receipt.body["disposition"], "rescheduled");
    assert_eq!(receipt.body["next_attempt_at"], "2026-01-01T01:15:00Z");
    assert_eq!(receipt.body["last_reason"], "source_timeout");
    assert_eq!(counts(maintenance.postgres_pool()).await?, (1, 2, 1, 2));
    Ok(())
}

pub(super) async fn auto_close_replay(
    store: &DatabaseStore,
    maintenance: &DatabaseStore,
) -> Result<(), WriteError> {
    seed_listing(store, maintenance).await?;
    let operation = Operation::AutoClose(ListingReason::ServiceDisabled);
    let changed = Operation::AutoClose(ListingReason::SellerIneligible);
    let first = execute(store, operation, RequestHash::new([7; 32])).await?;
    let receipt = verify_replay(store, operation, changed, first).await?;
    assert_eq!(receipt.body["disposition"], "closed");
    assert_eq!(receipt.body["reason_code"], "service_disabled");
    assert_eq!(receipt.body["listing_version"], 2);
    assert_eq!(counts(maintenance.postgres_pool()).await?, (1, 2, 2, 2));
    Ok(())
}
