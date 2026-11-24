//! 将命令、卖家、装备与原信键的会话锁置于 Serializable 快照之前。

use std::collections::BTreeSet;
use std::time::Duration;

use auction_application::consignment::ConsignmentError;
use auction_application::core::TransactionPolicy;
use auction_domain::consignment::ListingEquipmentIdentity;
use auction_domain::consignment::OriginalMailKey;
use futures_util::future::BoxFuture;
use sha2::{Digest, Sha256};
use sqlx::postgres::Postgres;
use sqlx::{Connection, PgConnection};
use tokio::time::error::Elapsed;
use tokio::time::{Instant, timeout, timeout_at};
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::backend::{retry_conflicts, set_read_only_limits};
use crate::database::consignment::commands::errors::{WriteError, commit_error};
use crate::database::pool::{TrackedConnection, TrackedTransaction};

/// 全局命令锁命名空间。
pub(crate) const COMMAND_LOCK_NAMESPACE: i32 = 0x4353_4701;
/// 本地用户锁命名空间。
pub(crate) const USER_LOCK_NAMESPACE: i32 = 0x4353_4702;
/// 规范实物 `(world,EID)` 锁命名空间；拍卖三入口与寄售共用这一 owner 常量。
pub(crate) const EQUIPMENT_LOCK_NAMESPACE: i32 = 0x4353_4703;
/// 稳定原信键 K 的锁命名空间；固定排在全部业务锁之后。
pub(crate) const MAIL_KEY_LOCK_NAMESPACE: i32 = 0x4353_4704;

const LOCK_BUDGET: Duration = Duration::from_secs(1);
const STATEMENT_BUDGET: Duration = Duration::from_secs(5);
const TRANSACTION_BUDGET: Duration = Duration::from_secs(30);

/// 取得全部会话锁之后、开启 Serializable 快照之前的测试钩子。
pub(crate) type AfterLockHook = dyn Fn() -> BoxFuture<'static, ()> + Send + Sync;

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
        let mut keys =
            BTreeSet::from([(COMMAND_LOCK_NAMESPACE, lock_hash(&[self.command_id.as_bytes()]))]);
        keys.extend(
            self.sellers
                .iter()
                .map(|seller| (USER_LOCK_NAMESPACE, lock_hash(&[seller.as_bytes()]))),
        );
        keys.extend(self.equipment.iter().map(equipment_lock_key));
        keys
    }

    /// v2 收件命令的完整锁集合：命令、排序去重的用户、规范实物，最后才是 K。
    pub(super) fn ordered_with_mail_keys(
        &self,
        mail_keys: &[OriginalMailKey],
    ) -> BTreeSet<(i32, i32)> {
        let mut keys = self.ordered();
        keys.extend(mail_keys.iter().map(|key| {
            (
                MAIL_KEY_LOCK_NAMESPACE,
                lock_hash(&[
                    key.authenticated_bot_uid().as_bytes(),
                    b"\0",
                    auction_domain::consignment::settlement_world_code(key.world()).as_bytes(),
                    b"\0",
                    key.external_mid().as_bytes(),
                ]),
            )
        }));
        keys
    }
}

/// 寄售与拍卖三入口共用的规范实物锁键；世界与 EID 以斜杠分隔后取 SHA-256 前四字节。
pub(crate) fn equipment_lock_key(identity: &ListingEquipmentIdentity) -> (i32, i32) {
    (
        EQUIPMENT_LOCK_NAMESPACE,
        lock_hash(&[
            identity.world_id().as_bytes(),
            b"/",
            identity.external_equipment_id().as_bytes(),
        ]),
    )
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
    pub(crate) fn connection(&mut self) -> &mut PgConnection {
        self.connection
    }
}

/// 寄售 v2 命令的同一顶层事务。闭包只能经此借用连接，不能另开池事务或保存点。
pub(crate) struct DepositTransaction<'tx, 'conn> {
    tracked: &'tx mut TrackedTransaction<'conn, Postgres>,
    pub(super) command_id: Uuid,
    pub(super) keys: &'tx LockKeys<'tx>,
    pub(super) mail_keys: &'tx [OriginalMailKey],
}

impl<'tx, 'conn> DepositTransaction<'tx, 'conn> {
    /// 供业务 SQL 与旧 owner 的事务内原语共用同一连接。
    pub(crate) fn connection(&mut self) -> &mut PgConnection {
        &mut *self.tracked
    }

    /// 旧退件案件与恢复原语要求的原生持有事务；仍借用本次顶层事务。
    pub(crate) fn tracked(&mut self) -> &mut TrackedTransaction<'conn, Postgres> {
        self.tracked
    }

    /// 命令身份。
    pub(crate) const fn command_id(&self) -> Uuid {
        self.command_id
    }

    /// 锁内重读后集合必须与预读相同；变化时退出，由调用方重新预读并重规划。
    ///
    /// 返回 [`ConsignmentError::Unavailable`] 表示本事务不能沿用旧锁集合或旧快照：
    /// 调用方不得在本次事务内补锁或继续判定，而应以同一 `command_id` 原样重试，
    /// 由下一次预读重新取得完整锁集合（D9 §1.3 的“退出重规划”）。
    pub(crate) fn ensure_lock_set(
        &self,
        sellers: &[Uuid],
        equipment: &[ListingEquipmentIdentity],
        mail_keys: &[OriginalMailKey],
    ) -> Result<(), WriteError> {
        let current_sellers: BTreeSet<_> = self.keys.sellers.iter().copied().collect();
        let expected_sellers: BTreeSet<_> = sellers.iter().copied().collect();
        let equipment_key = |identity: &ListingEquipmentIdentity| {
            (identity.world_id().to_owned(), identity.external_equipment_id().to_owned())
        };
        let current_equipment: BTreeSet<_> =
            self.keys.equipment.iter().map(equipment_key).collect();
        let expected_equipment: BTreeSet<_> = equipment.iter().map(equipment_key).collect();
        let current_mail_keys: BTreeSet<_> = self.mail_keys.iter().collect();
        let expected_mail_keys: BTreeSet<_> = mail_keys.iter().collect();
        if current_sellers != expected_sellers
            || current_equipment != expected_equipment
            || current_mail_keys != expected_mail_keys
        {
            // 合同 §1.3：锁键集合在预读后变化时必须退出本事务重新规划，不能沿用旧快照。
            return Err(ConsignmentError::Unavailable.into());
        }
        Ok(())
    }
}

/// 只在事务内执行有界的本地数据库操作。结果未知时调用方查原命令或原样重试。
/// 闭包只借用事务连接，不能开启保存点、提交、向外部服务发请求或发布过程内副作用。
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

/// v2 收件与代管的顶层事务：同一专用连接先取全部锁，再开 Serializable 快照。
pub(crate) async fn with_deposit_transaction<T, C>(
    store: &DatabaseStore,
    keys: LockKeys<'_>,
    mail_keys: &[OriginalMailKey],
    input: &C,
    operation: impl for<'tx> Fn(
        &'tx C,
        &'tx mut DepositTransaction<'_, '_>,
    ) -> BoxFuture<'tx, Result<T, WriteError>>,
) -> Result<T, WriteError> {
    with_deposit_transaction_hooked(store, keys, mail_keys, input, None, operation).await
}

/// 仅测试在取锁与开快照之间插入屏障；生产入口固定为 `None`。
pub(crate) async fn with_deposit_transaction_hooked<T, C>(
    store: &DatabaseStore,
    keys: LockKeys<'_>,
    mail_keys: &[OriginalMailKey],
    input: &C,
    after_locks: Option<&AfterLockHook>,
    operation: impl for<'tx> Fn(
        &'tx C,
        &'tx mut DepositTransaction<'_, '_>,
    ) -> BoxFuture<'tx, Result<T, WriteError>>,
) -> Result<T, WriteError> {
    if keys.command_id.is_nil() {
        return Err(ConsignmentError::InvalidCommandId.into());
    }
    let ordered = keys.ordered_with_mail_keys(mail_keys);
    retry_conflicts(
        store.pool(),
        store.transaction_owner,
        usize::from(TransactionPolicy::serializable().max_attempts),
        |error| matches!(error, WriteError::RetryConflict),
        || attempt_deposit(store, &keys, mail_keys, &ordered, input, after_locks, &operation),
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
    // SQLx 也关闭这条物理连接。会话级 statement_timeout / lock_timeout 也要求
    // 该连接不回池，此标记不可撤销，正常路径仍显式逐把释放锁。
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

async fn attempt_deposit<T, C>(
    store: &DatabaseStore,
    keys: &LockKeys<'_>,
    mail_keys: &[OriginalMailKey],
    ordered: &BTreeSet<(i32, i32)>,
    input: &C,
    after_locks: Option<&AfterLockHook>,
    operation: impl for<'tx> Fn(
        &'tx C,
        &'tx mut DepositTransaction<'_, '_>,
    ) -> BoxFuture<'tx, Result<T, WriteError>>,
) -> Result<T, WriteError> {
    let mut connection = prepare_connection(store).await?;
    let acquired = acquire_locks(&mut connection, ordered).await?;
    if let Some(after_locks) = after_locks {
        after_locks().await;
    }
    let result = transact_deposit(&mut connection, keys, mail_keys, input, operation).await;
    release_locks(&mut connection, acquired).await;
    result
}

async fn prepare_connection(
    store: &DatabaseStore,
) -> Result<TrackedConnection<Postgres>, WriteError> {
    let mut connection = store.pool().acquire().await?;
    // 标记必须早于发送第一条取锁 SQL。即使 future 取消、panic 或服务器结果未知，
    // SQLx 也关闭这条物理连接。会话级 statement_timeout / lock_timeout 也要求
    // 该连接不回池，此标记不可撤销，正常路径仍显式逐把释放锁。
    connection.close_on_drop();
    bounded(STATEMENT_BUDGET, async {
        sqlx::query("SELECT set_config('statement_timeout', '5s', false), set_config('lock_timeout', '1s', false)")
            .execute(&mut *connection)
            .await?;
        Ok(())
    })
    .await?;
    Ok(connection)
}

async fn acquire_locks(
    connection: &mut TrackedConnection<Postgres>,
    ordered: &BTreeSet<(i32, i32)>,
) -> Result<Vec<(i32, i32)>, WriteError> {
    let mut acquired = Vec::with_capacity(ordered.len());
    let lock_deadline = Instant::now() + LOCK_BUDGET;
    for &(namespace, key) in ordered {
        match timeout_at(lock_deadline, take_lock(connection, namespace, key)).await {
            Ok(Ok(())) => acquired.push((namespace, key)),
            Ok(Err(error)) => {
                release_locks(connection, acquired).await;
                return Err(error.into());
            }
            // 尚未确认这一次取锁的结果，由已标记的连接关闭解除全部会话锁。
            Err(Elapsed { .. }) => return Err(ConsignmentError::Unavailable.into()),
        }
    }
    Ok(acquired)
}

async fn take_lock(
    connection: &mut TrackedConnection<Postgres>,
    namespace: i32,
    key: i32,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_lock($1, $2)")
        .bind(namespace)
        .bind(key)
        .execute(&mut **connection)
        .await
        .map(drop)
}

async fn release_locks(connection: &mut TrackedConnection<Postgres>, acquired: Vec<(i32, i32)>) {
    let result = bounded(STATEMENT_BUDGET, async {
        for (namespace, key) in acquired.into_iter().rev() {
            let released: bool = sqlx::query_scalar("SELECT pg_advisory_unlock($1, $2)")
                .bind(namespace)
                .bind(key)
                .fetch_one(&mut **connection)
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

async fn transact_deposit<T, C>(
    connection: &mut TrackedConnection<Postgres>,
    keys: &LockKeys<'_>,
    mail_keys: &[OriginalMailKey],
    input: &C,
    operation: impl for<'tx> Fn(
        &'tx C,
        &'tx mut DepositTransaction<'_, '_>,
    ) -> BoxFuture<'tx, Result<T, WriteError>>,
) -> Result<T, WriteError> {
    let deadline = Instant::now() + TRANSACTION_BUDGET;
    let sqlx_transaction = bounded(STATEMENT_BUDGET, async {
        // 不变量 2：在命令、用户、实物与 K 全部取得之后才开始 Serializable。
        Ok(connection.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await?)
    })
    .await?;
    let mut tracked = TrackedTransaction::borrowed(sqlx_transaction);
    let result = timeout_at(deadline, async {
        set_read_only_limits(&mut tracked)
            .await
            .map_err(|(_setting, error)| WriteError::from(error))?;
        sqlx::query("SET LOCAL transaction_timeout = '30s'").execute(&mut *tracked).await?;
        operation(
            input,
            &mut DepositTransaction {
                tracked: &mut tracked,
                command_id: keys.command_id,
                keys,
                mail_keys,
            },
        )
        .await
    })
    .await
    .unwrap_or_else(|_elapsed| Err(ConsignmentError::Unavailable.into()));

    match result {
        Ok(value) => {
            timeout_at(deadline, tracked.commit())
                .await
                .map_err(|Elapsed { .. }| WriteError::from(ConsignmentError::OutcomeUnknown))?
                .map_err(commit_error)?;
            Ok(value)
        }
        Err(error) => {
            // 不变量 3、8：仅确认回滚后才允许重试。失败时连接已标记关闭。
            if let Err(rollback_error) =
                bounded(STATEMENT_BUDGET, async { Ok(tracked.rollback().await?) }).await
            {
                tracing::warn!(error = %rollback_error, "无法确认寄售 v2 事务已回滚，连接将关闭");
                return Err(ConsignmentError::Unavailable.into());
            }
            Err(error)
        }
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

    #[test]
    fn deposit_lock_keys_put_the_mail_key_group_last() {
        let seller = Uuid::from_u128(1);
        let equipment = ListingEquipmentIdentity::new("main", "42").expect("已知装备身份");
        let mail_key = auction_domain::consignment::OriginalMailKey::new(
            "12345",
            auction_domain::SettlementWorld::Main,
            "678",
        )
        .expect("规范原信键");
        let command_id = Uuid::from_u128(2);
        let keys = LockKeys {
            command_id,
            sellers: &[seller],
            equipment: std::slice::from_ref(&equipment),
        };
        let ordered: Vec<_> = keys.ordered_with_mail_keys(&[mail_key]).into_iter().collect();
        let namespaces: Vec<_> = ordered.iter().map(|(namespace, _)| *namespace).collect();
        assert!(namespaces.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(namespaces.last(), Some(&super::MAIL_KEY_LOCK_NAMESPACE));
    }
}
