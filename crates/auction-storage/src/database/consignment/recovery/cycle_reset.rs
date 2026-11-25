//! 解禁后的买家周期重置（`reset_cycle` 系统命令）：周期原语与账本回执同事务，同 ID 同请求回放原结果。
//!
//! 保护不变量 1、2、3、8：命令 ID 由原账号解禁事实派生，同 ID 异请求冲突；调用方（解禁入口）必须已持有买家用户锁，
//! 周期关闭 / 开启与账本回执同事务提交；旧周期的 pending 处罚转 `cycle_closed`、已落效果的处罚保持原状（只追加）。
//! 生产调用方是 Admin 解禁（`accounts/users/admin/postgres/commands/unblacklist.rs`）。

use auction_application::consignment::RecoveryOperation;
use auction_application::core::RequestHash;
use auction_domain::UserId;
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use super::ledger::{Arbitration, LedgerCommand, Step, Target, arbitrate, insert_success};
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::refusal::{CycleTransition, close_cycle_and_open_next};

/// 解禁重置命令；ID 与摘要由应用层的 `reset_cycle_command` 产生。
pub(crate) struct ResetCycle<'a> {
    /// 系统命令 UUIDv5。
    pub(crate) command_id: Uuid,
    /// 规范摘要。
    pub(crate) request_hash: &'a RequestHash,
    /// 被解禁的买家。
    pub(crate) buyer: UserId,
}

/// 在持有买家锁的顶层事务连接上重置周期，并写（或回放）账本回执。
pub(crate) async fn reset_cycle_in_conn(
    conn: &mut PgConnection,
    command: &ResetCycle<'_>,
) -> Result<CycleTransition, WriteError> {
    let ledger = LedgerCommand {
        command_id: command.command_id,
        operation: RecoveryOperation::ResetCycle,
        target: Target::BuyerCycle(command.buyer.as_uuid()),
        digest: command.request_hash,
    };
    match arbitrate(conn, &ledger).await? {
        Step::Done(Arbitration::Replay(body)) => return transition_of(&body),
        Step::Done(Arbitration::First) => {}
        Step::Rejected(rejection) => return Err(WriteError::from(rejection.into_error())),
    }
    let transition = close_cycle_and_open_next(conn, command.buyer).await?;
    let receipt = json!({
        "buyer_user_id": command.buyer.as_uuid(),
        "closed_cycle_id": transition.closed_cycle_id,
        "opened_cycle_id": transition.opened_cycle_id,
        "closed_sanction_id": transition.closed_sanction_id,
    });
    insert_success(conn, &ledger, "cycle_opened", receipt).await?;
    Ok(transition)
}

/// 从已存回执还原转移结果；形状不符是持久数据损坏，失败关闭。
fn transition_of(body: &Value) -> Result<CycleTransition, WriteError> {
    let uuid_of = |key: &str| -> Result<Option<Uuid>, WriteError> {
        match body.get(key) {
            Some(Value::Null) => Ok(None),
            Some(Value::String(text)) => Uuid::parse_str(text).map(Some).map_err(|error| {
                tracing::warn!(%error, "reset_cycle 回执里的 UUID 无法解析");
                WriteError::invariant("consignment_reset_cycle_receipt")
            }),
            _ => Err(WriteError::invariant("consignment_reset_cycle_receipt")),
        }
    };
    let opened = uuid_of("opened_cycle_id")?
        .ok_or_else(|| WriteError::invariant("consignment_reset_cycle_receipt"))?;
    Ok(CycleTransition {
        closed_cycle_id: uuid_of("closed_cycle_id")?,
        opened_cycle_id: opened,
        closed_sanction_id: uuid_of("closed_sanction_id")?,
    })
}

#[cfg(feature = "test-db")]
impl crate::database::DatabaseStore {
    /// 测试入口：按生产锁序（命令与买家用户锁在先、Serializable 快照在后）执行 `reset_cycle`；
    /// `hook` 在取锁之后、开快照之前运行。命令 ID 同时是命令锁键。
    pub async fn test_reset_cycle(
        &self,
        command_id: auction_domain::CommandId,
        request_hash: RequestHash,
        buyer: UserId,
        hook: Option<&crate::database::consignment::refusal::test_access::TestAfterLock>,
    ) -> Result<CycleTransition, auction_application::consignment::ConsignmentError> {
        use crate::database::consignment::commands::session::{
            LockKeys, with_deposit_transaction_hooked,
        };
        let users = [buyer.as_uuid()];
        let keys = LockKeys { command_id: command_id.as_uuid(), sellers: &users, equipment: &[] };
        let command = (command_id.as_uuid(), request_hash, buyer);
        with_deposit_transaction_hooked(self, keys, &[], &command, hook, |command, tx| {
            Box::pin(async move {
                let reset = ResetCycle {
                    command_id: command.0,
                    request_hash: &command.1,
                    buyer: command.2,
                };
                reset_cycle_in_conn(tx.connection(), &reset).await
            })
        })
        .await
        .map_err(WriteError::into_application)
    }
}
