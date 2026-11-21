//! 显式同步的 PostgreSQL 会话锁、快照和取消场景。

use std::future::pending;
use std::time::Duration;

use auction_application::consignment::{ConsignmentError, ListingOperation};
use auction_application::core::RequestHash;
use auction_domain::consignment::{ListingChange, ListingSnapshot, ListingVersion};
use sqlx::PgPool;
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use uuid::Uuid;

use super::fixture::{
    COMMAND, LISTING, OTHER_SELLER, SELLER, counts, equipment, later, price, register, seed,
};
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::facts::Decision;
use crate::database::consignment::commands::ledger::{
    Actor, Arbitration, RequestProof, StoredReceipt, Target,
};
use crate::database::consignment::commands::session::{
    CommandTransaction, LockKeys, with_command_transaction,
};
use crate::database::consignment::listings::projection::lock_listings;

fn bare(command_id: Uuid) -> LockKeys<'static> {
    LockKeys { command_id, sellers: &[], equipment: &[] }
}

fn digest() -> RequestProof {
    RequestProof::Digest(RequestHash::new([71; 32]))
}

async fn wait_advisory_waiter(pool: &PgPool) -> Result<(), WriteError> {
    timeout(Duration::from_millis(900), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_locks
                  WHERE locktype = 'advisory' AND NOT granted AND objsubid = 2 AND
                        database = (SELECT oid FROM pg_database WHERE datname = current_database()))",
            )
            .fetch_one(pool)
            .await?;
            if waiting {
                return Ok(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_elapsed| panic!("第二条事务应在 PostgreSQL 等待会话锁"))
}

async fn join<T>(task: JoinHandle<Result<T, WriteError>>) -> Result<T, WriteError> {
    task.await.unwrap_or_else(|error| panic!("夹具事务应正常结束：{error}"))
}

async fn reprice(
    tx: &mut CommandTransaction<'_>,
    seller: Uuid,
    listing_id: Uuid,
) -> Result<(bool, StoredReceipt), WriteError> {
    match tx
        .arbitrate(
            Actor::PublicUser(seller),
            ListingOperation::Reprice,
            Target::Listing(listing_id),
        )
        .await?
    {
        Arbitration::Replay(replay) => Ok((true, replay.verify(digest())?)),
        Arbitration::First(mut writer) => {
            let rows = lock_listings(writer.connection(), &[listing_id]).await?;
            let [current] = rows.as_slice() else {
                panic!("应锁住测试挂单");
            };
            writer
                .stage_change(
                    current,
                    ListingSnapshot {
                        state_version: ListingVersion::new(2)
                            .unwrap_or_else(|error| panic!("测试版本：{error}")),
                        price: price(125),
                        ..current.snapshot
                    },
                    None,
                    ListingChange::Repriced,
                    later(),
                )
                .await?;
            Ok((false, writer.finish(digest(), Decision::Changed(listing_id), later()).await?))
        }
    }
}

async fn race_one_command(
    store: &DatabaseStore,
    maintenance: &DatabaseStore,
    other_target: Uuid,
) -> Result<(StoredReceipt, Result<(bool, StoredReceipt), WriteError>), WriteError> {
    seed(maintenance).await?;
    register(store, Uuid::from_u128(801), LISTING, SELLER, "42").await?;
    let (other_seller, other_eid) = if other_target == LISTING {
        (SELLER, "42")
    } else {
        register(store, Uuid::from_u128(802), other_target, OTHER_SELLER, "43").await?;
        (OTHER_SELLER, "43")
    };
    let first_store = store.clone();
    let (entered, mut xid_receiver) = mpsc::channel::<String>(1);
    let release = std::sync::Arc::new(Notify::new());
    let first_release = release.clone();
    let first = tokio::spawn(async move {
        with_command_transaction(
            &first_store,
            LockKeys { command_id: COMMAND, sellers: &[SELLER], equipment: &[equipment("42")] },
            &(entered, first_release),
            |(entered, release), tx| {
                Box::pin(async move {
                    let (replayed, receipt) = reprice(tx, SELLER, LISTING).await?;
                    assert!(!replayed);
                    let xid = sqlx::query_scalar("SELECT pg_current_xact_id()::text")
                        .fetch_one(&mut *tx.connection)
                        .await?;
                    entered
                        .try_send(xid)
                        .unwrap_or_else(|error| panic!("应交付首写事务身份：{error}"));
                    release.notified().await;
                    Ok(receipt)
                })
            },
        )
        .await
    });
    let first_xid = timeout(Duration::from_secs(5), xid_receiver.recv())
        .await
        .unwrap_or_else(|_elapsed| panic!("夹具第一条事务未到达同步点"))
        .unwrap_or_else(|| panic!("夹具第一条事务未交付事务身份"));
    let second_store = store.clone();
    let second = tokio::spawn(async move {
        with_command_transaction(
            &second_store,
            LockKeys {
                command_id: COMMAND,
                sellers: &[other_seller],
                equipment: &[equipment(other_eid)],
            },
            &first_xid,
            move |first_xid, tx| {
                Box::pin(async move {
                    let sees_first: bool = sqlx::query_scalar(
                        "SELECT pg_visible_in_snapshot($1::text::xid8, pg_current_snapshot())",
                    )
                    .bind(first_xid)
                    .fetch_one(&mut *tx.connection)
                    .await?;
                    assert!(sees_first, "等待者的首次 Serializable 快照必须看到已提交的首写事务");
                    reprice(tx, other_seller, other_target).await
                })
            },
        )
        .await
    });
    wait_advisory_waiter(maintenance.postgres_pool()).await?;
    release.notify_one();
    Ok((join(first).await?, join(second).await))
}

pub(super) async fn command_replay(
    store: &DatabaseStore,
    maintenance: &DatabaseStore,
) -> Result<(), WriteError> {
    let (first, second) = race_one_command(store, maintenance, LISTING).await?;
    let (replayed, second) = second?;
    assert!(replayed);
    assert_eq!(first, second);
    assert_eq!(first.status, 200);
    assert_eq!(first.body["price"], 125);
    assert_eq!(counts(maintenance.postgres_pool()).await?, (1, 2, 2, 2));
    Ok(())
}

pub(super) async fn command_dual_target(
    store: &DatabaseStore,
    maintenance: &DatabaseStore,
) -> Result<(), WriteError> {
    let (_, second) = race_one_command(store, maintenance, Uuid::from_u128(999)).await?;
    assert!(matches!(second, Err(WriteError::Application(ConsignmentError::CommandIdConflict))));
    assert_eq!(counts(maintenance.postgres_pool()).await?, (2, 3, 3, 3));
    Ok(())
}

async fn race_keys(
    store: &DatabaseStore,
    pool: &PgPool,
    first_sellers: &'static [Uuid],
    second_sellers: &'static [Uuid],
    first_eids: &'static [&'static str],
    second_eids: &'static [&'static str],
) -> Result<(), WriteError> {
    let entered = std::sync::Arc::new(Notify::new());
    let release = std::sync::Arc::new(Notify::new());
    let first_entered = entered.clone();
    let first_release = release.clone();
    let first_store = store.clone();
    let first = tokio::spawn(async move {
        let equipment: Vec<_> = first_eids.iter().map(|eid| equipment(eid)).collect();
        with_command_transaction(
            &first_store,
            LockKeys { command_id: COMMAND, sellers: first_sellers, equipment: &equipment },
            &(first_entered, first_release),
            |(entered, release), tx| {
                Box::pin(async move {
                    let isolation: String = sqlx::query_scalar("SHOW transaction_isolation")
                        .fetch_one(&mut *tx.connection)
                        .await?;
                    assert_eq!(isolation, "serializable");
                    entered.notify_one();
                    release.notified().await;
                    Ok(())
                })
            },
        )
        .await
    });
    timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap_or_else(|_elapsed| panic!("夹具第一条事务未到达同步点"));
    let second_store = store.clone();
    let second = tokio::spawn(async move {
        let equipment: Vec<_> = second_eids.iter().map(|eid| equipment(eid)).collect();
        with_command_transaction(
            &second_store,
            LockKeys {
                command_id: Uuid::from_u128(302),
                sellers: second_sellers,
                equipment: &equipment,
            },
            &(),
            |_, _tx| Box::pin(async { Ok(()) }),
        )
        .await
    });
    wait_advisory_waiter(pool).await?;
    release.notify_one();
    join(first).await?;
    join(second).await
}

pub(super) async fn same_seller(
    store: &DatabaseStore,
    maintenance: &DatabaseStore,
) -> Result<(), WriteError> {
    race_keys(store, maintenance.postgres_pool(), &[SELLER], &[SELLER], &["42"], &["43"]).await
}

pub(super) async fn same_equipment(
    store: &DatabaseStore,
    maintenance: &DatabaseStore,
) -> Result<(), WriteError> {
    race_keys(store, maintenance.postgres_pool(), &[SELLER], &[OTHER_SELLER], &["42"], &["42"])
        .await
}

pub(super) async fn reversed_keys(
    store: &DatabaseStore,
    maintenance: &DatabaseStore,
) -> Result<(), WriteError> {
    race_keys(
        store,
        maintenance.postgres_pool(),
        &[SELLER, OTHER_SELLER, SELLER],
        &[OTHER_SELLER, SELLER],
        &["42", "43", "42"],
        &["43", "42"],
    )
    .await
}

pub(super) async fn session_cleanup(
    store: &DatabaseStore,
    maintenance: &DatabaseStore,
) -> Result<(), WriteError> {
    for mode in ["error", "panic", "cancel"] {
        let entered = std::sync::Arc::new(Notify::new());
        let task_entered = entered.clone();
        let task_store = store.clone();
        let task = tokio::spawn(async move {
            with_command_transaction(&task_store, bare(COMMAND), &task_entered, |entered, _tx| {
                Box::pin(async move {
                    entered.notify_one();
                    match mode {
                        "error" => Err(WriteError::from(ConsignmentError::Disabled)),
                        "panic" => panic!("寄售锁清理测试的预定 panic"),
                        _ => pending::<Result<(), WriteError>>().await,
                    }
                })
            })
            .await
        });
        timeout(Duration::from_secs(5), entered.notified())
            .await
            .unwrap_or_else(|_elapsed| panic!("夹具第一条事务未到达同步点"));
        if mode == "cancel" {
            task.abort();
        }
        match (mode, task.await) {
            ("error", Ok(Err(WriteError::Application(ConsignmentError::Disabled)))) => {}
            ("panic", Err(error)) if error.is_panic() => {}
            ("cancel", Err(error)) if error.is_cancelled() => {}
            (mode, result) => panic!("夹具应走预定路径 {mode}：{result:?}"),
        }
        timeout(
            Duration::from_secs(2),
            with_command_transaction(store, bare(COMMAND), &(), |_, _tx| {
                Box::pin(async { Ok(()) })
            }),
        )
        .await
        .unwrap_or_else(|_elapsed| panic!("同一池应立即重新取得命令锁"))?;
        let pool = maintenance.postgres_pool();
        let any_lock: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_locks
              WHERE locktype = 'advisory' AND objsubid = 2 AND
                    database = (SELECT oid FROM pg_database WHERE datname = current_database()))",
        )
        .fetch_one(pool)
        .await?;
        assert!(!any_lock, "结束后 PostgreSQL 不应持有寄售会话锁");
    }
    Ok(())
}
