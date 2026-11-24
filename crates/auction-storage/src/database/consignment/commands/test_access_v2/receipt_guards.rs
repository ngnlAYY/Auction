//! v2 有界重试、回执与暂存事件一致性及非重试错误的测试入口。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use auction_application::consignment::{
    ConsignmentError, DepositActor, DepositOperation, EnvelopeDisposition, EnvelopeReceipt,
    ListingStateChange,
};
use auction_application::core::RequestHash;
use auction_domain::CommandId;
use auction_domain::consignment::ListingState;
use auction_time::Timestamp;
use uuid::Uuid;

use super::super::errors::WriteError;
use super::super::ledger::{DepositArbitration, DepositTarget};
use super::super::session::{LockKeys, with_deposit_transaction};
use super::TestDepositLocks;
use super::TestDepositOutcome;
use super::TestDepositReceipt;
use super::TestDepositTarget;
use super::primitives::fixture_now;
use super::receipt::any;
use crate::database::DatabaseStore;

impl DatabaseStore {
    /// 第一次尝试确定触发 `40001`，第二次以同一输入成功；调用方用计数器断言重试次数。
    #[allow(clippy::too_many_arguments, reason = "测试入口逐个显式传入夹具字段")]
    pub async fn test_deposit_serialization_conflict_retries(
        &self,
        locks: TestDepositLocks,
        actor: DepositActor,
        operation: DepositOperation,
        target: TestDepositTarget,
        request_hash: [u8; 32],
        receipt: TestDepositReceipt,
        probe_id: Uuid,
        attempts: Arc<AtomicUsize>,
    ) -> Result<TestDepositOutcome, String> {
        let store = self.clone();
        let keys = LockKeys {
            command_id: locks.command_id,
            sellers: &locks.sellers,
            equipment: &locks.equipment,
        };
        let target: DepositTarget = target.into();
        let input = (actor, operation, target, request_hash, receipt, probe_id, attempts);
        with_deposit_transaction(self, keys, &locks.mail_keys, &input, move |input, tx| {
            let store = store.clone();
            Box::pin(async move {
                let (actor, operation, target, request_hash, receipt, probe_id, attempts) = input;
                let attempt = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                if attempt == 1 {
                    // 先读目标行，再让另一连接改写并提交，最后本事务写同一行。
                    // Serializable 会确定返回 40001；若未返回，测试必须失败而不是继续写回执。
                    let original: Timestamp = sqlx::query_scalar(
                        "SELECT updated_at FROM public.consignment_replay_keys WHERE id = $1",
                    )
                    .bind(*probe_id)
                    .fetch_one(tx.connection())
                    .await?;
                    let changed =
                        Timestamp::parse_from_rfc3339("2026-01-01T05:00:00Z").ok().ok_or_else(
                            || WriteError::invariant("consignment_deposit_retry_fixture_time"),
                        )?;
                    let mut other = store.postgres_pool().begin().await?;
                    sqlx::query(
                        "UPDATE public.consignment_replay_keys SET updated_at = $2 WHERE id = $1",
                    )
                    .bind(*probe_id)
                    .bind(changed)
                    .execute(&mut *other)
                    .await?;
                    other.commit().await?;
                    let affected = sqlx::query(
                        "UPDATE public.consignment_replay_keys SET updated_at = $2 WHERE id = $1",
                    )
                    .bind(*probe_id)
                    .bind(original)
                    .execute(tx.connection())
                    .await?;
                    if affected.rows_affected() != 1 {
                        return Err(WriteError::invariant("consignment_deposit_retry_probe_row"));
                    }
                    return Err(WriteError::invariant(
                        "consignment_deposit_retry_conflict_expected",
                    ));
                }
                match tx
                    .arbitrate(
                        actor.clone(),
                        *operation,
                        target.clone(),
                        RequestHash::new(*request_hash),
                        None,
                    )
                    .await?
                {
                    DepositArbitration::Replay(stored) => {
                        Ok(TestDepositOutcome::Replay(stored.into()))
                    }
                    DepositArbitration::First(mut writer) => {
                        let now = fixture_now()?;
                        if let Some((listing, version, from, to, kind, reason)) =
                            receipt.trigger_event()
                        {
                            writer
                                .stage_synthetic_event(
                                    listing, version, from, to, kind, reason, now,
                                )
                                .await?;
                        }
                        let stored = writer.finish(any(receipt), now).await?;
                        Ok(TestDepositOutcome::First(stored.into()))
                    }
                }
            })
        })
        .await
        .map_err(super::primitives::error_text)
    }

    /// 闭包只执行一次并返回调用方给定的非重试错误。
    pub async fn test_deposit_single_attempt_error(
        &self,
        locks: TestDepositLocks,
        attempts: Arc<AtomicUsize>,
        error: ConsignmentError,
    ) -> Result<(), String> {
        let keys = LockKeys {
            command_id: locks.command_id,
            sellers: &locks.sellers,
            equipment: &locks.equipment,
        };
        with_deposit_transaction(
            self,
            keys,
            &locks.mail_keys,
            &(attempts, error),
            |(attempts, error), _tx| {
                Box::pin(async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Err(WriteError::from(*error))
                })
            },
        )
        .await
        .map_err(super::primitives::error_text)
    }

    /// 暂存一条事件后提交与事件版本或状态不一致的回执；`finish` 必须拒绝并整体回滚。
    #[allow(clippy::too_many_arguments, reason = "测试入口逐个显式传入夹具字段")]
    pub async fn test_deposit_finish_with_mismatched_listing_change(
        &self,
        locks: TestDepositLocks,
        actor: DepositActor,
        operation: DepositOperation,
        target: TestDepositTarget,
        request_hash: [u8; 32],
        listing_id: Uuid,
        event_version: u64,
        receipt_changes: Vec<ListingStateChange>,
    ) -> Result<(), String> {
        let keys = LockKeys {
            command_id: locks.command_id,
            sellers: &locks.sellers,
            equipment: &locks.equipment,
        };
        let target: DepositTarget = target.into();
        let input =
            (actor, operation, target, request_hash, listing_id, event_version, receipt_changes);
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
                    DepositArbitration::Replay(_) => {
                        Err(WriteError::invariant("consignment_deposit_mismatch_replay"))
                    }
                    DepositArbitration::First(mut writer) => {
                        let now = fixture_now()?;
                        writer
                            .stage_synthetic_event(
                                input.4,
                                input.5,
                                Some(ListingState::AwaitingDeposit),
                                ListingState::DepositAccepted,
                                "deposit_accepted",
                                None,
                                now,
                            )
                            .await?;
                        let mut receipt = EnvelopeReceipt::new(
                            CommandId::from_uuid(writer.tx.command_id),
                            EnvelopeDisposition::Accepted,
                            Uuid::from_u128(0x7f01),
                        );
                        receipt.listing_changes = input.6.clone();
                        writer.finish(receipt, now).await?;
                        Ok(())
                    }
                }
            })
        })
        .await
        .map_err(super::primitives::error_text)
    }
}
