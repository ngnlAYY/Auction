//! 寄售购买与 v2 改价的存储适配（第 6 期 6A2）。
//!
//! [`PurchaseRepository`] 实现 `PurchaseStore` 与 `RepriceV2Store`：每个写操作一个顶层事务，
//! 先取命令锁、买卖双方用户锁与规范实物锁，再开 Serializable，序列化冲突与死锁有界重试，
//! 提交结果未知时调用方以同一命令 ID 重放读取原回执。购买事务在一次提交里写 Sale、义务图
//! （`write_graph_in_conn`）、首轮（`create_first_round_in_tx`）、position `committed`、挂单
//! `sale_pending`、事件与回执；买家处罚阻断经 `buyer_sanction_pending` 在同一买家锁下重验。
//! 资金预检与首轮邮件内容由装配根注入，存储层不渲染邮件、不写资金表。

mod content;
mod facts;
mod ledger;
mod reprice;
#[cfg(feature = "test-db")]
pub mod test_access;
mod transaction;

use std::sync::Arc;

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, PurchaseCommand, PurchaseOutcome, PurchaseStore,
    RepriceV2Command, RepriceV2Outcome, RepriceV2Store,
};
use auction_application::settlement::ConsignmentFundingPreflight;
use auction_domain::consignment::{ConsignmentSalePolicy, parse_settlement_world};

pub use self::content::{
    FirstDispatchContent, FirstDispatchContentSource, FirstDispatchRequest,
    WithdrawalDispatchRequest,
};
use self::ledger::{Arbitration, CommandIdentity, Operation, arbitrate};
use self::reprice::{RepriceCall, reprice_in_tx};
pub(crate) use self::transaction::decode_owned_purchase;
use self::transaction::{PurchaseCall, decode_replay, lock_equipment, lock_users, purchase_in_tx};
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::{
    AfterLockHook, LockKeys, with_deposit_transaction_hooked,
};
use crate::database::consignment::listings::projection::pre_read_listing_identity;

/// 购买与 v2 改价的 PostgreSQL 端口实现。
///
/// `policy` 是启动时冻结的 Sale 恢复策略；每笔成交把它写进合同，之后配置变化不改写已成交合同。
#[derive(Clone)]
pub struct PurchaseRepository {
    store: DatabaseStore,
    preflight: Arc<dyn ConsignmentFundingPreflight>,
    content: Arc<dyn FirstDispatchContentSource>,
    policy: ConsignmentSalePolicy,
}

impl PurchaseRepository {
    /// 组合存储、购前资金预检、首轮邮件内容来源与冻结的 Sale 策略。
    pub fn new(
        store: DatabaseStore,
        preflight: Arc<dyn ConsignmentFundingPreflight>,
        content: Arc<dyn FirstDispatchContentSource>,
        policy: ConsignmentSalePolicy,
    ) -> Self {
        Self { store, preflight, content, policy }
    }

    async fn run_purchase(
        &self,
        command: PurchaseCommand,
        after_locks: Option<&AfterLockHook>,
    ) -> Result<PurchaseOutcome, ConsignmentError> {
        if command.input.listing_id.is_nil() {
            return Err(ConsignmentError::InvalidListingId);
        }
        let identity = pre_read_listing_identity(&self.store, command.input.listing_id)
            .await
            .map_err(WriteError::into_application)?;
        let users = lock_users(command.buyer_user_id, identity.as_ref().map(|i| i.seller_user_id));
        let equipment = lock_equipment(identity.as_ref().map(|identity| &identity.equipment));
        let keys = LockKeys {
            command_id: command.command_id.as_uuid(),
            sellers: &users,
            equipment: &equipment,
        };
        // 资金预检在开事务、取会话锁之前读：它经连接池另取连接，持锁时再取会等待池、有耗尽连接池的死锁风险。
        // 预检只读、不取资金锁，买后条件变化本就只保留权利；读取失败留到锁内真正需要事实时才失败，
        // 使同命令 ID 的重放与命令冲突不受预检可用性影响。
        let funding = match identity
            .as_ref()
            .and_then(|identity| parse_settlement_world(identity.equipment.world_id()))
        {
            Some(world) => Some(self.preflight.preflight(world).await),
            None => None,
        };
        let call = PurchaseCall {
            repository: self,
            command: &command,
            pre_read_seller: identity.as_ref().map(|identity| identity.seller_user_id),
            funding,
        };
        with_deposit_transaction_hooked(&self.store, keys, &[], &call, after_locks, |call, tx| {
            Box::pin(purchase_in_tx(call, tx))
        })
        .await
        .map_err(WriteError::into_application)
    }
}

impl PurchaseStore for PurchaseRepository {
    fn purchase(&self, command: PurchaseCommand) -> ConsignmentFuture<'_, PurchaseOutcome> {
        Box::pin(self.run_purchase(command, None))
    }

    /// 只读快照里按命令 ID 仲裁：不取任何锁，不写账本，因而不受开关、资格与资金预检影响。
    /// 已提交的回执不可变，所以无锁的快照读足以回放原结果。
    fn replay_purchase(
        &self,
        command: PurchaseCommand,
    ) -> ConsignmentFuture<'_, Option<PurchaseOutcome>> {
        Box::pin(async move {
            let mut tx = self
                .store
                .pool()
                .begin_read_snapshot()
                .await
                .map_err(|error| WriteError::from(error).into_application())?;
            let identity = CommandIdentity {
                command_id: command.command_id.as_uuid(),
                user_id: command.buyer_user_id.as_uuid(),
                operation: Operation::Purchase,
                listing_id: command.input.listing_id,
                digest: command.request_hash,
                request_id: command.request_id.as_deref(),
            };
            let outcome = match arbitrate(&mut tx, &identity).await {
                Ok(Arbitration::Replay(stored)) => Some(decode_replay(&stored, command.command_id)),
                Ok(Arbitration::First) => None,
                Err(error) => return Err(error.into_application()),
            };
            tx.commit().await.map_err(|error| WriteError::from(error).into_application())?;
            outcome.transpose().map_err(WriteError::into_application)
        })
    }
}

impl RepriceV2Store for PurchaseRepository {
    fn reprice(&self, command: RepriceV2Command) -> ConsignmentFuture<'_, RepriceV2Outcome> {
        Box::pin(async move {
            if command.input.listing_id.is_nil() {
                return Err(ConsignmentError::InvalidListingId);
            }
            let identity = pre_read_listing_identity(&self.store, command.input.listing_id)
                .await
                .map_err(WriteError::into_application)?;
            let users =
                lock_users(command.seller_user_id, identity.as_ref().map(|i| i.seller_user_id));
            let equipment = lock_equipment(identity.as_ref().map(|identity| &identity.equipment));
            let keys = LockKeys {
                command_id: command.command_id.as_uuid(),
                sellers: &users,
                equipment: &equipment,
            };
            let call = RepriceCall {
                command: &command,
                pre_read_seller: identity.as_ref().map(|identity| identity.seller_user_id),
            };
            with_deposit_transaction_hooked(&self.store, keys, &[], &call, None, |call, tx| {
                Box::pin(reprice_in_tx(call, tx))
            })
            .await
            .map_err(WriteError::into_application)
        })
    }
}
