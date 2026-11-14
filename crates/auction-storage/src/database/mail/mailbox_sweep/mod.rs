//! MoogleMail 维护的本地证据与受证明恢复；状态、资金、履约及追加审计同事务提交。
//!
//! `candidates` 拥有未证明发送的只读投影，`transition` 拥有受证明迁移事务，
//! `error` 持有重试分类；本模块只保留端口实现与证据写入入口。

use auction_application::auction::ApplicationFuture;
use auction_application::delivery::{
    DeliverySendCredentialBinding, DeliverySendExpectation, DeliveryTakeExpectation,
};
use auction_application::mail::{
    MailScanPageObservation, MoogleMailboxEvidenceStore, MoogleMailboxSweepCandidate,
};
use serde_json::Value;
use sqlx::Postgres;
use uuid::Uuid;

use crate::database::storage_error;
use crate::database::storage_error_from;
use crate::database::{DatabaseStore, TrackedPool};

mod candidates;
mod error;
mod recovery;
mod transition;

use candidates::unproven_sends;
use error::RecoveryError;
pub(in crate::database) use recovery::resume_settlement_send;
use transition::{RecoveryUpdate, persist_transition};

impl MoogleMailboxEvidenceStore for DatabaseStore {
    fn unproven_sends<'a>(
        &'a self,
        world_id: &'a str,
        limit: i64,
    ) -> ApplicationFuture<'a, Vec<MoogleMailboxSweepCandidate>> {
        Box::pin(async move {
            if !matches!(world_id, "main" | "isekai") || !(1..=500).contains(&limit) {
                return Err(storage_error("MoogleMail 维护查询参数无效"));
            }
            unproven_sends(self.pool(), world_id, limit).await
        })
    }

    fn record_sweep_attempt<'a>(
        &'a self,
        world_id: &'a str,
        delivery_id: Uuid,
    ) -> ApplicationFuture<'a, bool> {
        Box::pin(async move {
            if !matches!(world_id, "main" | "isekai") || delivery_id.is_nil() {
                return Err(storage_error("MoogleMail 维护轮转参数无效"));
            }
            record_sweep_attempt(self.pool(), world_id, delivery_id).await
        })
    }

    fn begin_compose_discard<'a>(
        &'a self,
        delivery_id: Uuid,
        fencing_version: i64,
        expectation: Option<&'a DeliverySendExpectation>,
    ) -> ApplicationFuture<'a, bool> {
        Box::pin(async move {
            if delivery_id.is_nil()
                || fencing_version <= 0
                || expectation.is_some_and(|expectation| !expectation.validate())
            {
                return Err(storage_error("MoogleMail 草稿丢弃围栏参数无效"));
            }
            begin_compose_discard(self.pool(), delivery_id, fencing_version, expectation).await
        })
    }

    fn record_sent_pages<'a>(
        &'a self,
        observation: &'a MailScanPageObservation,
    ) -> ApplicationFuture<'a, ()> {
        Box::pin(async move {
            if !observation.validate() {
                return Err(storage_error("MoogleMail Sent 页数观察参数无效"));
            }
            sqlx::query(
                "UPDATE public.mail_scan_checkpoints
                     SET last_full_pages_scanned = $5,
                         last_full_page_limit = $7,
                         last_full_pages_observed_at = $6
                     WHERE credential_id = $1 AND credential_version = $2
                       AND world_id = $3 AND endpoint_scope = $4 AND stream = 'sent'
                       AND (last_full_pages_observed_at IS NULL
                            OR last_full_pages_observed_at <= $6)",
            )
            .bind(observation.credential_id)
            .bind(observation.credential_version)
            .bind(&observation.world_id)
            .bind(&observation.endpoint_scope)
            .bind(i32::try_from(observation.pages_scanned).unwrap_or(i32::MAX))
            .bind(observation.observed_at)
            .bind(i32::try_from(observation.page_limit).unwrap_or(i32::MAX))
            .execute(self.pool())
            .await
            .map_err(|error| storage_error_from(error, "写入 MoogleMail Sent 页数观察失败"))?;
            Ok(())
        })
    }

    fn record_sent_observation<'a>(
        &'a self,
        delivery_id: Uuid,
        fencing_version: i64,
        observation: Value,
    ) -> ApplicationFuture<'a, ()> {
        Box::pin(async move {
            if delivery_id.is_nil() || fencing_version <= 0 || !observation.is_object() {
                return Err(storage_error("MoogleMail Sent 观察参数无效"));
            }
            record_observation(
                self.pool(),
                delivery_id,
                fencing_version,
                "mailbox_sweep",
                observation,
            )
            .await
        })
    }

    fn record_sent_sighting<'a>(
        &'a self,
        delivery_id: Uuid,
        fencing_version: i64,
        external_mail_id: &'a str,
        observation: Value,
    ) -> ApplicationFuture<'a, ()> {
        Box::pin(async move {
            if delivery_id.is_nil()
                || fencing_version <= 0
                || !auction_domain::valid_positive_decimal(external_mail_id, 64)
                || !observation.is_object()
            {
                return Err(storage_error("MoogleMail Sent 目击参数无效"));
            }
            record_sighting(
                self.pool(),
                delivery_id,
                fencing_version,
                external_mail_id,
                observation,
            )
            .await
        })
    }

    fn record_compose_discard<'a>(
        &'a self,
        delivery_id: Uuid,
        fencing_version: i64,
        observation: Value,
    ) -> ApplicationFuture<'a, ()> {
        Box::pin(async move {
            if delivery_id.is_nil() || fencing_version <= 0 || !observation.is_object() {
                return Err(storage_error("MoogleMail 草稿丢弃证据参数无效"));
            }
            record_observation(
                self.pool(),
                delivery_id,
                fencing_version,
                "compose_discard",
                observation,
            )
            .await
        })
    }

    fn bind_recovered_send<'a>(
        &'a self,
        delivery_id: Uuid,
        fencing_version: i64,
        external_mail_id: String,
        metadata: Value,
        expectation: Option<&'a DeliverySendExpectation>,
    ) -> ApplicationFuture<'a, bool> {
        Box::pin(async move {
            if delivery_id.is_nil()
                || fencing_version <= 0
                || external_mail_id.is_empty()
                || external_mail_id.len() > 64
                || !external_mail_id.bytes().all(|byte| byte.is_ascii_digit())
                || !metadata.is_object()
                || expectation.is_some_and(|expectation| !expectation.validate())
            {
                return Err(storage_error("MoogleMail 恢复证明参数无效"));
            }
            bind_recovered_send(
                self.pool(),
                delivery_id,
                fencing_version,
                external_mail_id,
                metadata,
                expectation,
            )
            .await
        })
    }

    fn bind_recovered_action<'a>(
        &'a self,
        delivery_id: Uuid,
        fencing_version: i64,
        external_mail_id: String,
        metadata: Value,
        expectation: &'a DeliveryTakeExpectation,
        observation: &'a DeliverySendCredentialBinding,
    ) -> ApplicationFuture<'a, bool> {
        Box::pin(async move {
            if delivery_id.is_nil()
                || fencing_version <= 0
                || external_mail_id.is_empty()
                || external_mail_id.len() > 64
                || !external_mail_id.bytes().all(|byte| byte.is_ascii_digit())
                || !metadata.is_object()
            {
                return Err(storage_error("MoogleMail 动作恢复证明参数无效"));
            }
            bind_recovered_action(
                self.pool(),
                delivery_id,
                fencing_version,
                external_mail_id,
                metadata,
                expectation,
                observation,
            )
            .await
        })
    }

    fn requeue_unproven<'a>(
        &'a self,
        delivery_id: Uuid,
        fencing_version: i64,
        retries: i64,
        reason: &'a str,
        expectation: &'a DeliveryTakeExpectation,
        observation: &'a DeliverySendCredentialBinding,
    ) -> ApplicationFuture<'a, bool> {
        Box::pin(async move {
            if delivery_id.is_nil()
                || fencing_version <= 0
                || !(0..=8).contains(&retries)
                || reason.is_empty()
            {
                return Err(storage_error("MoogleMail 维护重排队参数无效"));
            }
            requeue_unproven(
                self.pool(),
                delivery_id,
                fencing_version,
                retries,
                reason,
                expectation,
                observation,
            )
            .await
        })
    }
}

async fn record_sweep_attempt(
    pool: &TrackedPool<Postgres>,
    world_id: &str,
    delivery_id: Uuid,
) -> Result<bool, auction_application::core::ApplicationError> {
    // 只推进当前一条；取消或提交应答丢失最多消费本条机会，后缀位置不受影响。
    // 排序键来自原 delivery，不接受调用方伪造时间，也不锁住远端读取期间的业务行。
    let result = sqlx::query(
        "INSERT INTO public.mailbox_sweep_cursors (world_id, last_created_at, last_delivery_id)
         SELECT $1, created_at, id FROM public.deliveries
         WHERE id = $2 AND COALESCE(payload_world, 'main') = $1
           AND status = 'manual_reconcile' AND external_reference IS NULL
           AND payload_operation IN ('send', 'take')
           AND sending_at IS NOT NULL AND completed_at IS NOT NULL
         ON CONFLICT (world_id) DO UPDATE
         SET last_created_at = EXCLUDED.last_created_at,
             last_delivery_id = EXCLUDED.last_delivery_id",
    )
    .bind(world_id)
    .bind(delivery_id)
    .execute(pool.raw())
    .await
    .map_err(|error| storage_error_from(error, "MoogleMail 维护轮转提交未确认"))?;
    Ok(result.rows_affected() == 1)
}

async fn begin_compose_discard(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    fencing_version: i64,
    expectation: Option<&DeliverySendExpectation>,
) -> Result<bool, auction_application::core::ApplicationError> {
    // 一次性尝试不是远端成功证明；仅明确中止的冲突可以重试，不重放未知提交。
    crate::database::retry_conflicts(
        pool,
        auction_observability::DatabaseTransactionOwner::MailboxSweep,
        crate::database::SERIALIZABLE_ATTEMPTS,
        RecoveryError::retryable,
        || recovery::begin_discard_once(pool, delivery_id, fencing_version, expectation),
    )
    .await
    .map_err(|error| error.into_application_error("MoogleMail 草稿丢弃围栏提交未确认"))
}

async fn bind_recovered_action(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    fencing_version: i64,
    external_mail_id: String,
    metadata: Value,
    expectation: &DeliveryTakeExpectation,
    observation: &DeliverySendCredentialBinding,
) -> Result<bool, auction_application::core::ApplicationError> {
    persist_transition(
        pool,
        delivery_id,
        RecoveryUpdate::Action(
            &external_mail_id,
            &metadata,
            fencing_version,
            expectation,
            observation,
        ),
    )
    .await
}

async fn requeue_unproven(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    fencing_version: i64,
    retries: i64,
    reason: &str,
    expectation: &DeliveryTakeExpectation,
    observation: &DeliverySendCredentialBinding,
) -> Result<bool, auction_application::core::ApplicationError> {
    persist_transition(
        pool,
        delivery_id,
        RecoveryUpdate::Requeue(retries, reason, fencing_version, expectation, observation),
    )
    .await
}

async fn record_observation(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    fencing_version: i64,
    key: &str,
    observation: Value,
) -> Result<(), auction_application::core::ApplicationError> {
    // 只记录当前尝试的安全观察；旧响应不能附着到管理员重试后的新 fence。
    sqlx::query(
        "UPDATE public.deliveries
         SET result_metadata = COALESCE(result_metadata, '{}'::jsonb)
             || jsonb_build_object($2::text, $3::jsonb)
         WHERE id = $1 AND fencing_version=$4 AND status = 'manual_reconcile'",
    )
    .bind(delivery_id)
    .bind(key)
    .bind(observation)
    .bind(fencing_version)
    .execute(pool.raw())
    .await
    .map_err(|error| storage_error_from(error, "MoogleMail 维护证据写入失败"))?;
    Ok(())
}

/// 目击只写入仍在人工核对的结算发送，且 MID 未绑定或引用于其他投递；重复写入以最新观察覆盖。
async fn record_sighting(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    fencing_version: i64,
    external_mail_id: &str,
    observation: Value,
) -> Result<(), auction_application::core::ApplicationError> {
    sqlx::query(
        "UPDATE public.deliveries AS delivery
         SET result_metadata = COALESCE(delivery.result_metadata, '{}'::jsonb)
             || jsonb_build_object('sent_reference_sighting', $3::jsonb)
         WHERE delivery.id = $1 AND delivery.fencing_version = $2
           AND delivery.status = 'manual_reconcile' AND delivery.origin_kind = 'settlement'
           AND delivery.payload_operation = 'send'
           AND NOT EXISTS (SELECT 1 FROM public.delivery_sent_bindings binding
               WHERE binding.world_id = COALESCE(delivery.payload_world, 'main')
                 AND binding.external_mail_id = $4 AND binding.delivery_id <> delivery.id)
           AND NOT EXISTS (SELECT 1 FROM public.deliveries other
               WHERE other.id <> delivery.id
                 AND other.external_reference =
                     'mooglemail:' || COALESCE(delivery.payload_world, 'main') || ':send:' || $4)",
    )
    .bind(delivery_id)
    .bind(fencing_version)
    .bind(observation)
    .bind(external_mail_id)
    .execute(pool.raw())
    .await
    .map_err(|error| storage_error_from(error, "MoogleMail Sent 目击写入失败"))?;
    Ok(())
}

async fn bind_recovered_send(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    fencing_version: i64,
    external_mail_id: String,
    metadata: Value,
    expectation: Option<&DeliverySendExpectation>,
) -> Result<bool, auction_application::core::ApplicationError> {
    persist_transition(
        pool,
        delivery_id,
        RecoveryUpdate::Send(&external_mail_id, &metadata, fencing_version, expectation),
    )
    .await
}
