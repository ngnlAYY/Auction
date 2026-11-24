//! v2 确定拒绝的测试入口；拒绝闭集与 000134 的结果码 CHECK 逐项对应。

use auction_application::consignment::{
    ConsignmentError, DepositActor, DepositBusinessRejection, DepositOperation,
};
use auction_application::core::RequestHash;

use super::super::errors::WriteError;
use super::super::ledger::DepositArbitration;
use super::super::session::{LockKeys, with_deposit_transaction};
use super::super::writer_v2::DepositRejection;
use super::TestDepositLocks;
use super::TestDepositOutcome;
use super::TestDepositTarget;
use super::primitives::fixture_now;
use crate::database::DatabaseStore;

/// 4A 与存储内的确定拒绝闭集；测试逐变体经真实 CHECK 落库。
#[derive(Debug, Clone, Copy)]
pub enum TestDepositRejection {
    /// 4A 的确定业务拒绝。
    Business(DepositBusinessRejection),
    /// 两世界活动额度已满。
    ListingLimitReached,
    /// 挂单已承诺给买家。
    EquipmentCommitted,
    /// 缺少可证明的代管。
    CustodyUnproven,
    /// 当前存在运营止售。
    OperationsHold,
    /// 新业务已关闭。
    Disabled,
}

impl From<TestDepositRejection> for DepositRejection {
    fn from(rejection: TestDepositRejection) -> Self {
        match rejection {
            TestDepositRejection::Business(rejection) => Self::Business(rejection),
            TestDepositRejection::ListingLimitReached => Self::ListingLimitReached,
            TestDepositRejection::EquipmentCommitted => Self::EquipmentCommitted,
            TestDepositRejection::CustodyUnproven => Self::CustodyUnproven,
            TestDepositRejection::OperationsHold => Self::OperationsHold,
            TestDepositRejection::Disabled => Self::Disabled,
        }
    }
}

impl DatabaseStore {
    /// 首次写入或重放一条确定拒绝；只写安全回执与拒绝审计，不写业务事件。
    pub async fn test_deposit_submit_rejection(
        &self,
        locks: TestDepositLocks,
        actor: DepositActor,
        operation: DepositOperation,
        target: TestDepositTarget,
        request_hash: [u8; 32],
        rejection: TestDepositRejection,
    ) -> Result<TestDepositOutcome, ConsignmentError> {
        let keys = LockKeys {
            command_id: locks.command_id,
            sellers: &locks.sellers,
            equipment: &locks.equipment,
        };
        let target: super::super::ledger::DepositTarget = target.into();
        let input = (actor, operation, target, request_hash, rejection);
        with_deposit_transaction(self, keys, &locks.mail_keys, &input, |input, tx| {
            Box::pin(async move {
                match tx
                    .arbitrate(
                        input.0.clone(),
                        input.1,
                        input.2.clone(),
                        RequestHash::new(input.3),
                        None,
                    )
                    .await?
                {
                    DepositArbitration::Replay(stored) => {
                        Ok(TestDepositOutcome::Replay(stored.into()))
                    }
                    DepositArbitration::First(writer) => {
                        let now = fixture_now()?;
                        let stored = writer.finish_rejection(input.4.into(), now).await?;
                        Ok(TestDepositOutcome::First(stored.into()))
                    }
                }
            })
        })
        .await
        .map_err(WriteError::into_application)
    }
}
