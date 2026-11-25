//! 首轮原语的 PG 测试窄入口；只在 `test-db` 构建。

use std::sync::Arc;

use auction_application::consignment::ConsignmentError;
use auction_domain::consignment::ConsignmentObligationGraph;
use auction_domain::consignment::dispatch::FrozenDispatchIdentity;
use uuid::Uuid;

use super::lease::{RoundLease, RoundLeaseRequest};
use super::{create_first_round_in_tx, create_withdrawal_round_in_tx};
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::{LockKeys, with_deposit_transaction};
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::consignment::purchase::FirstDispatchContentSource;
use crate::database::consignment::rights::register_contract_rights_in_tx;

/// 测试事务的输入：购买前置 SQL、合同图、冻结身份与购买后置 SQL。
struct Input<'a> {
    before: &'a str,
    graph: &'a ConsignmentObligationGraph,
    frozen: &'a FrozenDispatchIdentity,
    after: &'a str,
}

impl DatabaseStore {
    /// 在一个顶层事务里依次执行前置 SQL（回执与合同图）、首轮原语与后置 SQL（position 与挂单），
    /// 模拟第 6 期购买事务的写序（含与生产购买同事务的权利登记）；返回 `(轮 ID, 投递 ID)`。
    /// 任一步失败整笔回滚。
    pub async fn test_purchase_with_first_round(
        &self,
        before_sql: &str,
        graph: &ConsignmentObligationGraph,
        frozen: &FrozenDispatchIdentity,
        after_sql: &str,
    ) -> Result<(Uuid, Uuid), ConsignmentError> {
        let keys = LockKeys {
            command_id: graph.contract.core().origin_command_id.as_uuid(),
            sellers: &[],
            equipment: &[],
        };
        let input = Input { before: before_sql, graph, frozen, after: after_sql };
        with_deposit_transaction(self, keys, &[], &input, |input, tx| {
            Box::pin(async move {
                sqlx::raw_sql(sqlx::AssertSqlSafe(input.before)).execute(tx.connection()).await?;
                // 生产购买在同一事务登记买卖双方权利；派发起写的受益人授权依赖它。
                let contract_id = input.graph.contract.core().contract_id.as_uuid();
                let now = database_now(tx.connection()).await?;
                register_contract_rights_in_tx(tx.connection(), contract_id, now).await?;
                let first =
                    create_first_round_in_tx(tx.tracked(), input.graph, input.frozen).await?;
                sqlx::raw_sql(sqlx::AssertSqlSafe(input.after)).execute(tx.connection()).await?;
                Ok::<_, WriteError>((first.round_id, first.delivery_id))
            })
        })
        .await
        .map_err(WriteError::into_application)
    }

    /// 在一个顶层事务里对已写入的合同图单独调用撤回件首轮原语（PIWX 反例入口）；任一步失败整笔回滚。
    pub async fn test_create_withdrawal_round(
        &self,
        graph: &ConsignmentObligationGraph,
        content: Arc<dyn FirstDispatchContentSource>,
    ) -> Result<(), ConsignmentError> {
        let keys = LockKeys {
            command_id: graph.contract.core().origin_command_id.as_uuid(),
            sellers: &[],
            equipment: &[],
        };
        let input = (graph, content);
        with_deposit_transaction(self, keys, &[], &input, |input, tx| {
            Box::pin(async move {
                create_withdrawal_round_in_tx(tx.tracked(), input.0, &input.1).await?;
                Ok::<_, WriteError>(())
            })
        })
        .await
        .map_err(WriteError::into_application)
    }

    /// 为以原始 SQL 写入的合同补登买卖双方权利（生产购买与合同图写入同事务登记），使派发起写的
    /// 受益人授权与生产一致。
    pub async fn test_register_contract_rights(
        &self,
        contract_id: Uuid,
    ) -> Result<(), ConsignmentError> {
        let keys = LockKeys { command_id: contract_id, sellers: &[], equipment: &[] };
        with_deposit_transaction(self, keys, &[], &contract_id, |contract_id, tx| {
            Box::pin(async move {
                let now = database_now(tx.connection()).await?;
                register_contract_rights_in_tx(tx.connection(), *contract_id, now).await?;
                Ok::<_, WriteError>(())
            })
        })
        .await
        .map_err(WriteError::into_application)
    }

    /// 投递租约的测试视图：取得租约为 `Ok((投递, 租约 token, fence))`，否则为固定分类码
    /// （`busy` / `settled` / `rejected`）。
    pub async fn test_lease_dispatch_round(
        &self,
        obligation_id: Uuid,
        claim_token: Uuid,
        claim_fencing: i64,
        round_id: Uuid,
    ) -> Result<Result<(Uuid, Uuid, u64), &'static str>, ConsignmentError> {
        let request = RoundLeaseRequest { obligation_id, claim_token, claim_fencing, round_id };
        Ok(match self.lease_dispatch_round(&request).await? {
            RoundLease::Leased { delivery_id, claim_token, fence } => {
                Ok((delivery_id, claim_token, fence))
            }
            RoundLease::Busy => Err("busy"),
            RoundLease::Settled => Err("settled"),
            RoundLease::Rejected => Err("rejected"),
        })
    }
}
