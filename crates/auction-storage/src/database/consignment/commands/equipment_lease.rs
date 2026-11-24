//! 拍卖准入的实物会话锁租约。
//!
//! 一条专用连接按规范 `(world, EID)` 锁键排序去重后逐把取得 session advisory lock。锁必须
//! 早于 Serializable 快照，并持有到业务事务、提交与提交后处理结束；正常结束逐把
//! `pg_advisory_unlock` 确认释放。取消、panic、取锁超时或任一次释放结果不明时关闭该会话，
//! 由 PostgreSQL 释放全部会话锁，绝不把可能仍持锁的连接放回池。

use std::collections::BTreeSet;
use std::time::Duration;

use auction_application::core::CompetitionTransactionError;
use auction_domain::consignment::ListingEquipmentIdentity;
use sqlx::Postgres;
use tokio::time::error::Elapsed;
use tokio::time::{Instant, sleep, timeout};

use super::session::equipment_lock_key;
use crate::database::TrackedConnection;
use crate::database::auction::competition::map_sql_error;
use crate::database::pool::TrackedPool;

/// 与寄售命令会话锁相同的取锁预算；预算内未取得就按暂时不可用返回。
const LOCK_BUDGET: Duration = Duration::from_secs(1);
const STATEMENT_BUDGET: Duration = Duration::from_secs(5);

/// 已取得的实物锁及其唯一专用连接。
struct EquipmentLockSession {
    connection: Option<TrackedConnection<Postgres>>,
    keys: Vec<(i32, i32)>,
}

impl EquipmentLockSession {
    fn new(connection: TrackedConnection<Postgres>) -> Self {
        Self { connection: Some(connection), keys: Vec::new() }
    }

    fn connection_mut(&mut self) -> Option<&mut TrackedConnection<Postgres>> {
        self.connection.as_mut()
    }

    /// 关闭连接；会话锁由服务端在连接断开时全部释放。
    fn close(&mut self) {
        if let Some(connection) = self.connection.as_mut() {
            connection.close_on_drop();
        }
        self.connection = None;
    }

    /// 逆序逐把确认解锁。任一结果不明都关闭连接，绝不让持锁连接回池。
    async fn unlock(&mut self) {
        let Some(connection) = self.connection.as_mut() else {
            return;
        };
        let result = timeout(STATEMENT_BUDGET, async {
            for &(namespace, key) in self.keys.iter().rev() {
                let unlocked: bool = sqlx::query_scalar("SELECT pg_advisory_unlock($1, $2)")
                    .bind(namespace)
                    .bind(key)
                    .fetch_one(&mut **connection)
                    .await
                    .map_err(map_sql_error)?;
                if !unlocked {
                    return Err(CompetitionTransactionError::Failed(
                        "实物会话锁释放结果不明".to_owned(),
                    ));
                }
            }
            Ok(())
        })
        .await;
        match result {
            Ok(Ok(())) => {
                // 全部确认释放后连接才可回池。
                self.connection = None;
            }
            Ok(Err(error)) => {
                tracing::warn!(%error, "实物会话锁释放结果不明，连接将关闭");
                self.close();
            }
            Err(Elapsed { .. }) => self.close(),
        }
    }
}

impl Drop for EquipmentLockSession {
    fn drop(&mut self) {
        // 取消、panic 或未走正常释放：关闭会话以释放全部 session advisory lock。
        self.close();
    }
}

/// 一组实物会话锁的租约；空集合不占连接。
pub(crate) struct EquipmentLease {
    session: Option<EquipmentLockSession>,
}

impl EquipmentLease {
    /// 取得全部规范实物锁；预算内任一锁未取得时释放（关闭）已取锁连接并返回
    /// [`CompetitionTransactionError::EquipmentLockUnavailable`]。
    pub(crate) async fn acquire(
        pool: &TrackedPool<Postgres>,
        equipment: &[ListingEquipmentIdentity],
    ) -> Result<Self, CompetitionTransactionError> {
        let ordered: BTreeSet<(i32, i32)> = equipment.iter().map(equipment_lock_key).collect();
        if ordered.is_empty() {
            return Ok(Self { session: None });
        }
        // 与出价队列相同：持锁连接使用独立锁连接配额，不占随后业务事务需要的普通配额，
        // 避免极小连接池里“持锁连接等事务连接、事务连接等配额”的自锁。
        let connection = pool.acquire_bid_queue().await.map_err(map_sql_error)?;
        let mut session = EquipmentLockSession::new(connection);
        let deadline = Instant::now() + LOCK_BUDGET;
        for (namespace, key) in ordered {
            // 用非阻塞 try-lock 轮询而不是服务器端等待：超时时连接处于空闲状态，关闭会话
            // 不会被仍在等待的 `pg_advisory_lock` 拖住，已取得的其他实物锁能立即释放。
            loop {
                let Some(connection) = session.connection_mut() else {
                    return Err(CompetitionTransactionError::EquipmentLockUnavailable);
                };
                let acquired = sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1, $2)")
                    .bind(namespace)
                    .bind(key)
                    .fetch_one(&mut **connection)
                    .await;
                match acquired {
                    Ok(true) => {
                        session.keys.push((namespace, key));
                        break;
                    }
                    Ok(false) if Instant::now() < deadline => {
                        sleep(Duration::from_millis(10)).await;
                    }
                    Ok(false) => {
                        session.close();
                        return Err(CompetitionTransactionError::EquipmentLockUnavailable);
                    }
                    Err(error) => {
                        tracing::warn!(
                            error = %crate::database::sql_error_context(
                                &error,
                                "实物会话锁取得失败"
                            ),
                            "实物会话锁取得失败，连接将关闭"
                        );
                        session.close();
                        return Err(CompetitionTransactionError::EquipmentLockUnavailable);
                    }
                }
            }
        }
        Ok(Self { session: Some(session) })
    }

    /// 正常结束：确认逐把释放后再归还连接；结果不明时关闭会话。
    pub(crate) async fn release(mut self) {
        if let Some(session) = self.session.as_mut() {
            session.unlock().await;
        }
        self.session = None;
    }
}
