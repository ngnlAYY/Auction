//! 拒收原语的 PG 测试窄入口；只在 `test-db` 构建。
//!
//! 每个入口按生产的锁序取买家用户锁（命令锁与用户锁在先、Serializable 快照在后），再调用原语；
//! `hook` 在取锁之后、开快照之前运行，用于在买家锁下交换提交顺序。

use auction_application::consignment::{ConsignmentError, StoredConsignmentGraph};
use auction_domain::UserId;
use auction_domain::consignment::ConsignmentObligationGraph;
use sqlx::PgConnection;
use uuid::Uuid;

pub use super::{
    CycleTransition, RefusalOutcome, ReturnEvidence, ReturnFactSubmission, SanctionRecord,
};
use super::{buyer_sanction_pending, close_cycle_and_open_next, submit_return_fact};
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::{LockKeys, with_deposit_transaction_hooked};
use crate::database::consignment::contracts::graph::write_graph_in_conn;

/// 取锁之后、开快照之前的测试钩子。
pub type TestAfterLock =
    dyn Fn() -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync;

/// 在调用方已开启的事务连接上写合同图（购买事务里回执、合同图、position 与挂单要同事务提交）。
pub async fn test_write_graph_in_conn(
    conn: &mut PgConnection,
    graph: &ConsignmentObligationGraph,
) -> Result<StoredConsignmentGraph, ConsignmentError> {
    write_graph_in_conn(conn, graph).await.map_err(WriteError::into_application)
}

impl DatabaseStore {
    /// 在持有 `locked_users` 用户锁的顶层事务内提交一条退回事实；锁集合里没有买家时应失败关闭。
    pub async fn test_refusal_submit(
        &self,
        command_id: Uuid,
        locked_users: &[UserId],
        submission: &ReturnFactSubmission,
        hook: Option<&TestAfterLock>,
    ) -> Result<RefusalOutcome, ConsignmentError> {
        self.run_locked(command_id, locked_users, hook, submission, |connection, submission| {
            Box::pin(async move { submit_return_fact(connection, submission).await })
        })
        .await
    }

    /// 在持有买家用户锁的顶层事务内走解禁路径：关闭旧周期并开启新周期。
    pub async fn test_refusal_close_cycle(
        &self,
        command_id: Uuid,
        buyer: UserId,
        hook: Option<&TestAfterLock>,
    ) -> Result<CycleTransition, ConsignmentError> {
        self.run_locked(command_id, &[buyer], hook, &buyer, |connection, buyer| {
            Box::pin(async move { close_cycle_and_open_next(connection, *buyer).await })
        })
        .await
    }

    /// 模拟购买事务的阻断重验：与 6A2 相同，买家与卖家同放用户锁集合，取锁后开快照再读阻断。
    pub async fn test_purchase_gate(
        &self,
        command_id: Uuid,
        buyer: UserId,
        seller: UserId,
        hook: Option<&TestAfterLock>,
    ) -> Result<bool, ConsignmentError> {
        self.run_locked(command_id, &[buyer, seller], hook, &buyer, |connection, buyer| {
            Box::pin(async move {
                buyer_sanction_pending(connection, *buyer).await.map_err(WriteError::from)
            })
        })
        .await
    }

    async fn run_locked<I: Sync, T: Send>(
        &self,
        command_id: Uuid,
        locked_users: &[UserId],
        hook: Option<&TestAfterLock>,
        input: &I,
        operation: impl for<'a> Fn(
            &'a mut PgConnection,
            &'a I,
        )
            -> futures_util::future::BoxFuture<'a, Result<T, WriteError>>,
    ) -> Result<T, ConsignmentError> {
        let users: Vec<Uuid> = locked_users.iter().copied().map(UserId::as_uuid).collect();
        let keys = LockKeys { command_id, sellers: &users, equipment: &[] };
        with_deposit_transaction_hooked(self, keys, &[], input, hook, |input, tx| {
            operation(tx.connection(), input)
        })
        .await
        .map_err(WriteError::into_application)
    }
}
