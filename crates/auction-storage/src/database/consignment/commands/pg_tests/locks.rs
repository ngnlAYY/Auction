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
use crate::database::consignment::commands::facts::{ChangedOperation, Decision};
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

fn digest() -> RequestProof<'static> {
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
            digest(),
        )
        .await?
    {
        Arbitration::Replay(receipt) => Ok((true, receipt)),
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
            Ok((
                false,
                writer
                    .finish(
                        Decision::Changed { listing_id, operation: ChangedOperation::Reprice },
                        later(),
                    )
                    .await?,
            ))
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

pub(super) async fn listing_row_is_locked(
    store: &DatabaseStore,
    maintenance: &DatabaseStore,
) -> Result<(), WriteError> {
    seed(maintenance).await?;
    register(store, COMMAND, LISTING, SELLER, "42").await?;
    let pool = maintenance.postgres_pool();
    with_command_transaction(
        store,
        LockKeys { command_id: Uuid::from_u128(302), sellers: &[SELLER], equipment: &[equipment("42")] },
        &pool,
        |pool, tx| Box::pin(async move {
            let owner_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *tx.connection).await?;
            let mut other = pool.acquire().await?;
            let other_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *other).await?;
            assert_ne!(owner_pid, other_pid);
            assert_eq!(lock_listings(tx.connection(), &[LISTING]).await?.len(), 1);
            let result: Result<Uuid, sqlx::Error> = sqlx::query_scalar(
                "SELECT id FROM public.consignment_listings WHERE id = $1 FOR UPDATE NOWAIT"
            ).bind(LISTING).fetch_one(&mut *other).await;
            assert!(
                matches!(&result, Err(error) if error.as_database_error().and_then(sqlx::error::DatabaseError::code).as_deref() == Some("55P03")),
                "独立连接必须因行锁拒绝 NOWAIT：{result:?}"
            );
            Ok(())
        }),
    ).await?;
    let row: Uuid = sqlx::query_scalar(
        "SELECT id FROM public.consignment_listings WHERE id = $1 FOR UPDATE NOWAIT",
    )
    .bind(LISTING)
    .fetch_one(pool)
    .await?;
    assert_eq!(row, LISTING);
    Ok(())
}

const COMMAND_NAMESPACE: i32 = 0x4353_4701;
const SELLER_NAMESPACE: i32 = 0x4353_4702;
const EQUIPMENT_NAMESPACE: i32 = 0x4353_4703;
const FIRST_SELLER: Uuid = Uuid::from_u128(1);
const SECOND_SELLER: Uuid = Uuid::from_u128(2);
const SELLER_LOCK: (i32, i32) = (SELLER_NAMESPACE, 1_764_255_177);
// 固定夹具对应的 PG 两参数锁键，按有符号键的契约顺序列出。
const BUSINESS_LOCKS: [(i32, i32); 4] = [
    SELLER_LOCK,
    (SELLER_NAMESPACE, 2_084_359_440),
    (EQUIPMENT_NAMESPACE, -1_157_983_326),
    (EQUIPMENT_NAMESPACE, 1_738_617_678),
];

#[derive(Debug, PartialEq, Eq)]
struct WaitingSession {
    pid: i32,
    held_command: Option<i32>,
    lock: (i32, i32),
}

async fn wait_sessions(pool: &PgPool, expected: usize) -> Result<Vec<WaitingSession>, WriteError> {
    timeout(Duration::from_millis(850), async {
        loop {
            let rows: Vec<(i32, Option<i64>, i32, i64)> = sqlx::query_as(
                "SELECT waiting.pid, command.objid::bigint, waiting.classid::int,
                        waiting.objid::bigint
                   FROM pg_locks waiting
                   LEFT JOIN pg_locks command ON command.pid = waiting.pid
                    AND command.locktype = 'advisory' AND command.objsubid = 2
                    AND command.classid::int = $1 AND command.granted
                  WHERE waiting.locktype = 'advisory' AND waiting.objsubid = 2
                    AND waiting.database = (SELECT oid FROM pg_database WHERE datname = current_database())
                    AND NOT waiting.granted ORDER BY waiting.pid",
            ).bind(COMMAND_NAMESPACE).fetch_all(pool).await?;
            if rows.len() == expected {
                return Ok(rows.into_iter().map(|(pid, held_command, namespace, key)| WaitingSession {
                    pid,
                    held_command: held_command.map(|key| u32::try_from(key).unwrap_or_else(|error| panic!("PG 锁键：{error}")).cast_signed()),
                    lock: (namespace, u32::try_from(key).unwrap_or_else(|error| panic!("PG 锁键：{error}")).cast_signed()),
                }).collect());
            }
            tokio::task::yield_now().await;
        }
    }).await.unwrap_or_else(|_elapsed| panic!("PG 应在取锁预算内观察到 {expected} 个等待 PID"))
}

async fn gate(
    connection: &mut sqlx::PgConnection,
    (namespace, key): (i32, i32),
    locked: bool,
) -> Result<(), WriteError> {
    if locked {
        sqlx::query("SELECT pg_advisory_lock($1, $2)")
            .bind(namespace)
            .bind(key)
            .execute(connection)
            .await?;
    } else {
        let unlocked: bool = sqlx::query_scalar("SELECT pg_advisory_unlock($1, $2)")
            .bind(namespace)
            .bind(key)
            .fetch_one(connection)
            .await?;
        assert!(unlocked);
    }
    Ok(())
}

fn ordered_attempt(
    store: &DatabaseStore,
    command_id: Uuid,
    sellers: [Uuid; 2],
    eids: [&'static str; 2],
) -> JoinHandle<Result<i32, WriteError>> {
    let store = store.clone();
    tokio::spawn(async move {
        let equipment = eids.map(equipment);
        with_command_transaction(
            &store,
            LockKeys { command_id, sellers: &sellers, equipment: &equipment },
            &(),
            |_, tx| {
                Box::pin(async move {
                    Ok(sqlx::query_scalar("SELECT pg_backend_pid()")
                        .fetch_one(tx.connection())
                        .await?)
                })
            },
        )
        .await
    })
}

pub(super) async fn actual_lock_order(
    store: &DatabaseStore,
    maintenance: &DatabaseStore,
) -> Result<(), WriteError> {
    let pool = maintenance.postgres_pool();
    let mut blocker = pool.acquire().await?;
    blocker.close_on_drop();

    // 先同时堵住两条反向输入的命令，让两条都显示各自实际请求的首把业务锁。
    for lock in BUSINESS_LOCKS {
        gate(&mut blocker, lock, true).await?;
    }
    let reverse_command = Uuid::from_u128(302);
    let first = ordered_attempt(store, COMMAND, [FIRST_SELLER, SECOND_SELLER], ["42", "43"]);
    let second =
        ordered_attempt(store, reverse_command, [SECOND_SELLER, FIRST_SELLER], ["43", "42"]);
    let waiting = wait_sessions(pool, 2).await?;
    assert!(
        waiting.iter().all(|session| session.lock == BUSINESS_LOCKS[0]),
        "实际首把业务锁不同：{waiting:?}"
    );
    assert_ne!(waiting[0].pid, waiting[1].pid);
    assert!(waiting.iter().all(|session| session.held_command.is_some()));
    for lock in BUSINESS_LOCKS {
        gate(&mut blocker, lock, false).await?;
    }
    join(first).await?;
    join(second).await?;

    // 按实际等待记录逐把放行，防止只检查集合或仅见到闭包里已拿齐的结果。
    for (command_id, sellers, eids) in [
        (COMMAND, [FIRST_SELLER, SECOND_SELLER], ["42", "43"]),
        (reverse_command, [SECOND_SELLER, FIRST_SELLER], ["43", "42"]),
    ] {
        for lock in BUSINESS_LOCKS {
            gate(&mut blocker, lock, true).await?;
        }
        let task = ordered_attempt(store, command_id, sellers, eids);
        let waiting = wait_sessions(pool, 1).await?;
        let pid = waiting[0].pid;
        let mut previous = None;
        for expected in BUSINESS_LOCKS {
            let running = timeout(Duration::from_millis(800), async {
                loop {
                    let sessions = wait_sessions(pool, 1).await?;
                    // 解锁上一个屏障后，PG 可能仍短暂报告上一把锁的等待记录。
                    if Some(sessions[0].lock) != previous {
                        return Ok::<_, WriteError>(sessions);
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap_or_else(|error| panic!("应依序等候下一把业务锁：{error}"))?;
            assert_eq!(running[0].pid, pid);
            assert_eq!(running[0].lock, expected, "实际取锁顺序不同");
            assert!(running[0].held_command.is_some(), "业务锁之前应已经取得命令锁");
            gate(&mut blocker, expected, false).await?;
            previous = Some(expected);
        }
        assert_eq!(join(task).await?, pid);
    }
    Ok(())
}

pub(super) async fn cancel_while_taking_locks(
    store: &DatabaseStore,
    maintenance: &DatabaseStore,
) -> Result<(), WriteError> {
    let pool = maintenance.postgres_pool();
    let mut blocker = pool.acquire().await?;
    blocker.close_on_drop();
    let blocker_pid: i32 =
        sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *blocker).await?;
    gate(&mut blocker, SELLER_LOCK, true).await?;
    let mut probe = pool.acquire().await?;
    let task = ordered_attempt(store, COMMAND, [FIRST_SELLER, SECOND_SELLER], ["42", "43"]);
    let sessions = wait_sessions(pool, 1).await?;
    let waiting = &sessions[0];
    assert_eq!(waiting.lock, SELLER_LOCK);
    let command_key = waiting.held_command.unwrap_or_else(|| panic!("等待卖家锁前必须持有命令锁"));
    let blocking: Vec<i32> = sqlx::query_scalar("SELECT pg_blocking_pids($1)")
        .bind(waiting.pid)
        .fetch_one(&mut *probe)
        .await?;
    assert_eq!(blocking, [blocker_pid]);

    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()), "取锁中的 future 应被取消");
    gate(&mut blocker, SELLER_LOCK, false).await?;

    let probe_pid: i32 =
        sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *probe).await?;
    assert_ne!(probe_pid, waiting.pid);
    assert_ne!(probe_pid, blocker_pid);
    // 探针每次都用非阻塞取锁，只给取消后的物理连接关闭留出调度余量。
    timeout(Duration::from_millis(750), async {
        loop {
            let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1, $2)")
                .bind(COMMAND_NAMESPACE)
                .bind(command_key)
                .fetch_one(&mut *probe)
                .await?;
            if acquired {
                return Ok::<_, WriteError>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|error| panic!("取消途中取锁后，第三条连接应能立即取得同一命令锁：{error}"))?;
    Ok(())
}
