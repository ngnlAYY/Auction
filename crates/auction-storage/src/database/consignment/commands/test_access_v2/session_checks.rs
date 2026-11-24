//! 会话锁归属、K 阻塞与专用连接关闭的测试入口。

use std::collections::BTreeSet;

use sqlx::Row;

use super::super::errors::WriteError;
use super::super::session::{LockKeys, with_deposit_transaction};
use super::TestDepositLocks;
use super::primitives::error_text;
use crate::database::DatabaseStore;

impl DatabaseStore {
    /// 返回规范锁序的实际 `(namespace,key)`，供测试在另一连接上占住某个 K。
    pub fn test_deposit_lock_keys(locks: &TestDepositLocks) -> Vec<(i32, i32)> {
        let keys = LockKeys {
            command_id: locks.command_id,
            sellers: &locks.sellers,
            equipment: &locks.equipment,
        };
        keys.ordered_with_mail_keys(&locks.mail_keys).into_iter().collect()
    }

    /// 在事务内确认命令、用户、实物与 K 的 advisory 锁全部由当前会话持有。
    pub async fn test_deposit_locks_owned_by_transaction_session(
        &self,
        locks: TestDepositLocks,
    ) -> Result<(), String> {
        let expected = Self::test_deposit_lock_keys(&locks);
        let keys = LockKeys {
            command_id: locks.command_id,
            sellers: &locks.sellers,
            equipment: &locks.equipment,
        };
        with_deposit_transaction(self, keys, &locks.mail_keys, &expected, |expected, tx| {
            Box::pin(async move {
                let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                    .fetch_one(tx.connection())
                    .await?;
                let rows = sqlx::query(
                    "SELECT classid::bigint AS namespace, objid::bigint AS lock_key
                     FROM pg_locks
                     WHERE locktype = 'advisory' AND pid = $1 AND granted",
                )
                .bind(pid)
                .fetch_all(tx.connection())
                .await?;
                let held: BTreeSet<(i32, i32)> = rows
                    .iter()
                    .map(|row| {
                        let namespace: i64 = row.get("namespace");
                        let lock_key: i64 = row.get("lock_key");
                        ((namespace as u64 as u32) as i32, (lock_key as u64 as u32) as i32)
                    })
                    .collect();
                for key in expected {
                    if !held.contains(key) {
                        return Err(WriteError::invariant(
                            "consignment_deposit_lock_session_owner",
                        ));
                    }
                }
                Ok(())
            })
        })
        .await
        .map_err(error_text)
    }

    /// 只按规范顺序取锁，用于另一连接占住 K 时的阻塞用例。
    pub async fn test_deposit_attempt_locks(&self, locks: TestDepositLocks) -> Result<(), String> {
        let keys = LockKeys {
            command_id: locks.command_id,
            sellers: &locks.sellers,
            equipment: &locks.equipment,
        };
        with_deposit_transaction(self, keys, &locks.mail_keys, &(), |(), _tx| {
            Box::pin(async { Ok(()) })
        })
        .await
        .map_err(error_text)
    }

    /// 在事务内读取后端 pid；命令结束后该 pid 必须离开 `pg_stat_activity`。
    pub async fn test_deposit_backend_pid(&self, locks: TestDepositLocks) -> Result<i32, String> {
        let keys = LockKeys {
            command_id: locks.command_id,
            sellers: &locks.sellers,
            equipment: &locks.equipment,
        };
        with_deposit_transaction(self, keys, &locks.mail_keys, &(), |(), tx| {
            Box::pin(async move {
                let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                    .fetch_one(tx.connection())
                    .await?;
                Ok(pid)
            })
        })
        .await
        .map_err(error_text)
    }
}
