//! v2 账本、回执往返与锁/连接场景的测试入口。

use std::sync::Arc;

use auction_application::consignment::{ConsignmentError, DepositActor, DepositOperation};
use auction_application::core::RequestHash;
use sqlx::Row;
use tokio::sync::Barrier;
use uuid::Uuid;

use super::super::errors::WriteError;
use super::super::ledger::{DepositArbitration, DepositTarget, StoredReceipt};
use super::super::session::{
    AfterLockHook, LockKeys, with_deposit_transaction, with_deposit_transaction_hooked,
};
use super::TestDepositLocks;
use super::TestDepositOutcome;
use super::TestDepositReceipt;
use super::TestDepositTarget;
use super::primitives::{error_text, fixture_now, tracked_marker};
use super::receipt::any;
use crate::database::DatabaseStore;
use crate::database::operations::audit_health::AuditWriteResult as _;

impl DatabaseStore {
    /// 首次写入或重放一条 v2 回执；相同 ID 异请求返回 `CommandIdConflict`。
    #[allow(clippy::too_many_arguments, reason = "测试入口逐个显式传入夹具字段")]
    pub async fn test_deposit_submit_receipt(
        &self,
        locks: TestDepositLocks,
        actor: DepositActor,
        operation: DepositOperation,
        target: TestDepositTarget,
        request_hash: [u8; 32],
        request_id: Option<String>,
        receipt: TestDepositReceipt,
    ) -> Result<TestDepositOutcome, ConsignmentError> {
        if receipt.operation() != operation {
            return Err(ConsignmentError::InvalidDepositRequest);
        }
        let keys = LockKeys {
            command_id: locks.command_id,
            sellers: &locks.sellers,
            equipment: &locks.equipment,
        };
        let target: DepositTarget = target.into();
        with_deposit_transaction(
            self,
            keys,
            &locks.mail_keys,
            &(actor, operation, target, request_hash, request_id, receipt),
            |(actor, operation, target, request_hash, request_id, receipt), tx| {
                Box::pin(async move {
                    match tx
                        .arbitrate(
                            actor.clone(),
                            *operation,
                            target.clone(),
                            RequestHash::new(*request_hash),
                            request_id.as_deref(),
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
            },
        )
        .await
        .map_err(WriteError::into_application)
    }

    /// 4A 值 → 序列化 → 000134 约束 → 读回解码；返回解码后的 4A 值。
    pub async fn test_deposit_roundtrip_receipt(
        &self,
        locks: TestDepositLocks,
        actor: DepositActor,
        target: TestDepositTarget,
        request_hash: [u8; 32],
        receipt: TestDepositReceipt,
    ) -> Result<TestDepositReceipt, ConsignmentError> {
        let command_id = locks.command_id;
        let keys = LockKeys { command_id, sellers: &locks.sellers, equipment: &locks.equipment };
        let target: DepositTarget = target.into();
        let operation = receipt.operation();
        with_deposit_transaction(
            self,
            keys,
            &locks.mail_keys,
            &(actor, operation, target, request_hash, receipt),
            |(actor, operation, target, request_hash, receipt), tx| {
                Box::pin(async move {
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
                        DepositArbitration::Replay(_) => {
                            Err(WriteError::invariant("consignment_deposit_roundtrip_replay"))
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
                            writer.finish_raw(receipt.status(), receipt.body(), now).await?;
                            let row = sqlx::query(
                                "SELECT result_status, result_code, result_body
                                 FROM public.consignment_commands WHERE command_id = $1",
                            )
                            .bind(command_id)
                            .fetch_one(tx.connection())
                            .await?;
                            let stored = StoredReceipt {
                                status: row.try_get("result_status")?,
                                code: row.try_get("result_code")?,
                                body: row.try_get("result_body")?,
                            };
                            TestDepositReceipt::decode(&stored)
                        }
                    }
                })
            },
        )
        .await
        .map_err(WriteError::into_application)
    }

    /// 返回规范锁序的命名空间标签，用于断言命令、用户、实物、K 的顺序。
    pub fn test_deposit_lock_order(locks: &TestDepositLocks) -> Vec<&'static str> {
        let keys = LockKeys {
            command_id: locks.command_id,
            sellers: &locks.sellers,
            equipment: &locks.equipment,
        };
        keys.ordered_with_mail_keys(&locks.mail_keys)
            .into_iter()
            .map(|(namespace, _)| match namespace {
                super::super::session::COMMAND_LOCK_NAMESPACE => "command",
                super::super::session::USER_LOCK_NAMESPACE => "user",
                super::super::session::EQUIPMENT_LOCK_NAMESPACE => "equipment",
                super::super::session::MAIL_KEY_LOCK_NAMESPACE => "mail_key",
                _ => "unknown",
            })
            .collect()
    }

    /// 取得规范锁序后停在屏障；两个连接反向传入同一组锁也不会死锁。
    pub async fn test_deposit_hold_locks(
        &self,
        locks: TestDepositLocks,
        barrier: Arc<Barrier>,
    ) -> Result<(), String> {
        let keys = LockKeys {
            command_id: locks.command_id,
            sellers: &locks.sellers,
            equipment: &locks.equipment,
        };
        let input = (locks.sellers.clone(), locks.equipment.clone(), locks.mail_keys.clone());
        barrier.wait().await;
        with_deposit_transaction(self, keys, &locks.mail_keys, &input, move |input, tx| {
            Box::pin(async move {
                tx.ensure_lock_set(&input.0, &input.1, &input.2)?;
                Ok(())
            })
        })
        .await
        .map_err(error_text)
    }

    /// 取锁后等待测试提交另一连接的事务，再开快照读取探针行数。
    pub async fn test_deposit_locks_before_snapshot(
        &self,
        locks: TestDepositLocks,
        probe_id: Uuid,
        acquired: Arc<Barrier>,
        committed: Arc<Barrier>,
    ) -> Result<u64, String> {
        let keys = LockKeys {
            command_id: locks.command_id,
            sellers: &locks.sellers,
            equipment: &locks.equipment,
        };
        let hook: Box<AfterLockHook> = Box::new(move || {
            let acquired = Arc::clone(&acquired);
            let committed = Arc::clone(&committed);
            Box::pin(async move {
                acquired.wait().await;
                committed.wait().await;
            })
        });
        with_deposit_transaction_hooked(
            self,
            keys,
            &locks.mail_keys,
            &probe_id,
            Some(hook.as_ref()),
            |probe_id, tx| {
                Box::pin(async move {
                    let count: i64 = sqlx::query_scalar(
                        "SELECT count(*) FROM public.consignment_replay_keys WHERE id = $1",
                    )
                    .bind(*probe_id)
                    .fetch_one(tx.connection())
                    .await?;
                    Ok(count.cast_unsigned())
                })
            },
        )
        .await
        .map_err(error_text)
    }

    /// 在寄售事务里调用需要 `TrackedTransaction` 的测试原语，并可强制整体回滚。
    pub async fn test_deposit_same_connection_marker(
        &self,
        locks: TestDepositLocks,
        tracked_id: Uuid,
        business_marker: Uuid,
        commit: bool,
    ) -> Result<(), String> {
        let keys = LockKeys {
            command_id: locks.command_id,
            sellers: &locks.sellers,
            equipment: &locks.equipment,
        };
        let markers = (tracked_id, business_marker);
        with_deposit_transaction(self, keys, &locks.mail_keys, &markers, |markers, tx| {
            Box::pin(async move {
                tracked_marker(tx.tracked(), markers.0).await?;
                sqlx::query(
                    "INSERT INTO public.audit_records
                        (id, actor_type, actor_id, action, entity_type, entity_id, result,
                         source, metadata)
                     VALUES ($1, 'system', 'deposit-test',
                             'test.consignment_deposit_business', 'consignment_command',
                             $1::text, 'success', 'system', '{}'::jsonb)",
                )
                .bind(markers.1)
                .execute(tx.connection())
                .await
                .record_audit_failure()?;
                let visible: bool = sqlx::query_scalar(
                    "SELECT count(*) = 2 FROM public.audit_records
                     WHERE id = ANY($1::uuid[])",
                )
                .bind(vec![markers.0, markers.1])
                .fetch_one(tx.connection())
                .await?;
                if !visible {
                    return Err(WriteError::invariant("consignment_deposit_second_transaction"));
                }
                if commit { Ok(()) } else { Err(ConsignmentError::Unavailable.into()) }
            })
        })
        .await
        .map_err(error_text)
    }
}
