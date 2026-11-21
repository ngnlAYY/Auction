//! 将命令、卖家、装备的会话锁置于 Serializable 快照之前。

use std::collections::BTreeSet;
use std::time::Duration;

use auction_application::consignment::ConsignmentError;
use auction_application::core::TransactionPolicy;
use auction_domain::consignment::ListingEquipmentIdentity;
use futures_util::future::BoxFuture;
use sha2::{Digest, Sha256};
use sqlx::{Connection, PgConnection};
use tokio::time::error::Elapsed;
use tokio::time::{Instant, timeout, timeout_at};
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::backend::{retry_conflicts, set_read_only_limits};
use crate::database::consignment::commands::errors::{WriteError, commit_error};

const COMMAND: i32 = 0x4353_4701;
const SELLER: i32 = 0x4353_4702;
const EQUIPMENT: i32 = 0x4353_4703;
const LOCK_BUDGET: Duration = Duration::from_secs(1);
const STATEMENT_BUDGET: Duration = Duration::from_secs(5);
const TRANSACTION_BUDGET: Duration = Duration::from_secs(30);

/// 由请求或无授权含义的预读准备的领域锁集合。
pub(crate) struct LockKeys<'a> {
    /// 整个寄售账本内唯一的命令身份。
    pub(crate) command_id: Uuid,
    /// 本次涉及的卖家，次序和重复由会话锁协议归一。
    pub(crate) sellers: &'a [Uuid],
    /// 需要保护的规范世界与 EID。
    pub(crate) equipment: &'a [ListingEquipmentIdentity],
}

impl LockKeys<'_> {
    fn ordered(&self) -> BTreeSet<(i32, i32)> {
        // 不变量 1、2：命令不按 actor/目标分片，业务键按各层实际锁键全集排序去重。
        // 同层哈希碰撞只让两个原本独立的身份串行。
        let mut keys = BTreeSet::from([(COMMAND, lock_hash(&[self.command_id.as_bytes()]))]);
        keys.extend(self.sellers.iter().map(|seller| (SELLER, lock_hash(&[seller.as_bytes()]))));
        keys.extend(self.equipment.iter().map(|identity| {
            (
                EQUIPMENT,
                lock_hash(&[
                    identity.world_id().as_bytes(),
                    b"/",
                    identity.external_equipment_id().as_bytes(),
                ]),
            )
        }));
        keys
    }
}

fn lock_hash(parts: &[&[u8]]) -> i32 {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update(part);
    }
    let [a, b, c, d, ..]: [u8; 32] = hash.finalize().into();
    i32::from_be_bytes([a, b, c, d])
}

/// 闭包在同一顶层事务中的借用。资格、投影、全局许可、工作和凭证依合同的行锁顺序读取。
pub(crate) struct CommandTransaction<'a> {
    pub(super) connection: &'a mut PgConnection,
    pub(super) command_id: Uuid,
    pub(super) keys: &'a LockKeys<'a>,
}

impl CommandTransaction<'_> {
    /// 首写调用 `finish` 或 `finish_registration` 后，可继续追加外键指向账本
    /// 的碰撞关联。保持同一事务，按 `arbitrate → finish → 追加关联` 的顺序访问。
    #[allow(dead_code, reason = "由后续任务 2D / 2F 追加碰撞关联")]
    pub(crate) fn connection(&mut self) -> &mut PgConnection {
        self.connection
    }
}

/// 只在事务内执行有界的本地数据库操作。结果未知时调用方查原命令或原样重试。
/// 闭包只借用事务连接，不能开启保存点、提交、向外部服务发请求或发布过程内副作用。
#[allow(dead_code, reason = "由后续任务 2D / 2F 调用")]
pub(crate) async fn with_command_transaction<T, C>(
    store: &DatabaseStore,
    keys: LockKeys<'_>,
    input: &C,
    operation: impl for<'tx> Fn(
        &'tx C,
        &'tx mut CommandTransaction<'_>,
    ) -> BoxFuture<'tx, Result<T, WriteError>>,
) -> Result<T, WriteError> {
    if keys.command_id.is_nil() {
        return Err(ConsignmentError::InvalidCommandId.into());
    }
    let ordered = keys.ordered();
    retry_conflicts(
        store.pool(),
        store.transaction_owner,
        usize::from(TransactionPolicy::serializable().max_attempts),
        |error| matches!(error, WriteError::RetryConflict),
        || attempt(store, &keys, &ordered, input, &operation),
    )
    .await
}

async fn attempt<T, C>(
    store: &DatabaseStore,
    keys: &LockKeys<'_>,
    ordered: &BTreeSet<(i32, i32)>,
    input: &C,
    operation: impl for<'tx> FnOnce(
        &'tx C,
        &'tx mut CommandTransaction<'_>,
    ) -> BoxFuture<'tx, Result<T, WriteError>>,
) -> Result<T, WriteError> {
    let mut connection = store.pool().acquire().await?;
    // 标记必须早于发送第一条取锁 SQL。即使 future 取消、panic 或服务器结果未知，
    // SQLx 也关闭这条物理连接。此标记不可撤销，正常路径仍显式逐把释放锁。
    connection.close_on_drop();
    bounded(STATEMENT_BUDGET, async {
        sqlx::query("SELECT set_config('statement_timeout', '5s', false), set_config('lock_timeout', '1s', false)")
            .execute(&mut *connection)
            .await?;
        Ok(())
    })
    .await?;

    let mut acquired = Vec::with_capacity(ordered.len());
    let lock_deadline = Instant::now() + LOCK_BUDGET;
    for &(namespace, key) in ordered {
        match timeout_at(lock_deadline, take_lock(&mut connection, namespace, key)).await {
            Ok(Ok(())) => acquired.push((namespace, key)),
            Ok(Err(error)) => {
                release_locks(&mut connection, acquired).await;
                return Err(error.into());
            }
            // 尚未确认这一次取锁的结果，由已标记的连接关闭解除全部会话锁。
            Err(Elapsed { .. }) => return Err(ConsignmentError::Unavailable.into()),
        }
    }

    let result = transact(&mut connection, keys, input, operation).await;
    release_locks(&mut connection, acquired).await;
    result
}

async fn take_lock(
    connection: &mut PgConnection,
    namespace: i32,
    key: i32,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_lock($1, $2)")
        .bind(namespace)
        .bind(key)
        .execute(connection)
        .await
        .map(drop)
}

async fn release_locks(connection: &mut PgConnection, acquired: Vec<(i32, i32)>) {
    let result = bounded(STATEMENT_BUDGET, async {
        for (namespace, key) in acquired.into_iter().rev() {
            let released: bool = sqlx::query_scalar("SELECT pg_advisory_unlock($1, $2)")
                .bind(namespace)
                .bind(key)
                .fetch_one(&mut *connection)
                .await?;
            if !released {
                return Err(WriteError::invariant("consignment_advisory_unlock"));
            }
        }
        Ok(())
    })
    .await;
    if let Err(error) = result {
        tracing::warn!(%error, "无法确认寄售会话锁已解除，连接将关闭");
    }
}

async fn transact<T, C>(
    connection: &mut PgConnection,
    keys: &LockKeys<'_>,
    input: &C,
    operation: impl for<'tx> FnOnce(
        &'tx C,
        &'tx mut CommandTransaction<'_>,
    ) -> BoxFuture<'tx, Result<T, WriteError>>,
) -> Result<T, WriteError> {
    let deadline = Instant::now() + TRANSACTION_BUDGET;
    let mut tx = bounded(STATEMENT_BUDGET, async {
        // 不变量 2：在三层锁全部取得之后才开始 Serializable。
        Ok(connection.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await?)
    })
    .await?;
    let result = timeout_at(deadline, async {
        set_read_only_limits(&mut tx).await.map_err(|(_setting, error)| WriteError::from(error))?;
        sqlx::query("SET LOCAL transaction_timeout = '30s'").execute(&mut *tx).await?;
        operation(
            input,
            &mut CommandTransaction { connection: &mut tx, command_id: keys.command_id, keys },
        )
        .await
    })
    .await
    .unwrap_or_else(|_elapsed| Err(ConsignmentError::Unavailable.into()));

    match result {
        Ok(value) => {
            timeout_at(deadline, tx.commit())
                .await
                .map_err(|Elapsed { .. }| WriteError::from(ConsignmentError::OutcomeUnknown))?
                .map_err(commit_error)?;
            Ok(value)
        }
        Err(error) => {
            // 不变量 3、8：仅确认回滚后才允许重试。失败时连接已标记关闭。
            if let Err(rollback_error) =
                bounded(STATEMENT_BUDGET, async { Ok(tx.rollback().await?) }).await
            {
                tracing::warn!(error = %rollback_error, "无法确认寄售事务已回滚，连接将关闭");
                return Err(ConsignmentError::Unavailable.into());
            }
            Err(error)
        }
    }
}

async fn bounded<T>(
    budget: Duration,
    future: impl Future<Output = Result<T, WriteError>>,
) -> Result<T, WriteError> {
    timeout(budget, future)
        .await
        .unwrap_or_else(|_elapsed| Err(ConsignmentError::Unavailable.into()))
}

#[cfg(test)]
mod tests {
    use auction_domain::consignment::ListingEquipmentIdentity;
    use uuid::Uuid;

    use super::LockKeys;

    #[test]
    fn consignment_lock_keys_have_fixed_namespaces_order_and_deduplication() {
        let main_42 = ListingEquipmentIdentity::new("main", "42").expect("已知装备身份");
        let main_43 = ListingEquipmentIdentity::new("main", "43").expect("已知装备身份");
        let isekai_42 = ListingEquipmentIdentity::new("isekai", "42").expect("已知装备身份");
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);
        let third = Uuid::from_u128(3);
        let keys = LockKeys {
            command_id: first,
            sellers: &[first, second, third, first],
            equipment: &[main_42.clone(), isekai_42, main_43, main_42],
        };
        let actual: Vec<_> = keys.ordered().into_iter().collect();
        assert_eq!(
            actual,
            vec![
                (0x4353_4701, 2_084_359_440),
                (0x4353_4702, -1_961_171_921),
                (0x4353_4702, 1_764_255_177),
                (0x4353_4702, 2_084_359_440),
                (0x4353_4703, -1_157_983_326),
                (0x4353_4703, -782_146_183),
                (0x4353_4703, 1_738_617_678),
            ]
        );
    }
}
