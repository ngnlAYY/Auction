//! MoogleMail 维护的本地证据与受证明恢复；状态、资金、履约及追加审计同事务提交。
//!
//! `candidates` 拥有未证明发送的只读投影，`transition` 拥有受证明迁移事务，`observation` 拥有
//! 结算退件 Take 原会话观察的留证事务，`error` 持有重试分类，`writes` 拥有巡检证据写入与迁移入口；
//! 本模块实现存储端口。
//! 全部 SQL 经受观测池的执行器与事务，占用普通业务配额，不借用 readiness 与长期 owner 的保留连接。

use auction_application::core::ApplicationFuture;
use auction_application::delivery::{
    DeliverySendCredentialBinding, DeliverySendExpectation, DeliveryTakeExpectation,
};
use auction_application::mail::{
    MailScanPageObservation, MoogleMailboxEvidenceStore, MoogleMailboxFailedSendSlot,
    MoogleMailboxSentEntry, MoogleMailboxSweepBatch, MoogleMailboxSweepPosition,
};
use auction_domain::SettlementReturnTakeUnknownReason;
use auction_time::Timestamp;
use serde_json::Value;
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::storage_error;
use crate::database::storage_error_from;

mod candidates;
mod error;
mod failed_sent;
mod observation;
pub(in crate::database) mod recovery;
mod transition;
mod writes;

use candidates::{record_sweep_attempt, unproven_sends};

impl MoogleMailboxEvidenceStore for DatabaseStore {
    fn unproven_sends<'a>(
        &'a self,
        world_id: &'a str,
        limit: i64,
    ) -> ApplicationFuture<'a, MoogleMailboxSweepBatch> {
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
            record_sweep_attempt(self.pool(), world_id, delivery_id, None, None, None, None).await
        })
    }
    fn record_sweep_attempt_from<'a>(
        &'a self,
        world_id: &'a str,
        delivery_id: Uuid,
        start: Option<MoogleMailboxSweepPosition>,
        sweep_id: Uuid,
        failed_fence: Option<i64>,
        oldest_visible: Option<Timestamp>,
    ) -> ApplicationFuture<'a, bool> {
        Box::pin(async move {
            if !matches!(world_id, "main" | "isekai")
                || delivery_id.is_nil()
                || sweep_id.is_nil()
                || failed_fence.is_some_and(|fence| fence <= 0)
                || (failed_fence.is_some() && oldest_visible.is_none())
            {
                return Err(storage_error("MoogleMail 维护轮转快照无效"));
            }
            record_sweep_attempt(
                self.pool(),
                world_id,
                delivery_id,
                start,
                Some(sweep_id),
                failed_fence,
                oldest_visible,
            )
            .await
        })
    }

    fn failed_sent_slots(
        &self,
        start: Option<MoogleMailboxSweepPosition>,
        oldest_visible: Timestamp,
        limit: i64,
    ) -> ApplicationFuture<'_, Vec<MoogleMailboxFailedSendSlot>> {
        Box::pin(async move {
            if !(1..=200).contains(&limit) {
                return Err(storage_error("MoogleMail 失败投递取证查询参数无效"));
            }
            failed_sent::read_slots(self.pool(), start, oldest_visible, limit).await
        })
    }

    fn begin_failed_sent_detail<'a>(
        &'a self,
        delivery_id: Uuid,
        fencing_version: i64,
        entry: Option<&'a MoogleMailboxSentEntry>,
    ) -> ApplicationFuture<'a, bool> {
        Box::pin(async move {
            if delivery_id.is_nil()
                || fencing_version <= 0
                || entry.is_some_and(|entry| {
                    !auction_domain::valid_positive_decimal(&entry.external_mail_id, 64)
                })
            {
                return Err(storage_error("MoogleMail 失败投递详情登记参数无效"));
            }
            failed_sent::begin_detail(self.pool(), delivery_id, fencing_version, entry).await
        })
    }
    fn begin_failed_sent_detail_from<'a>(
        &'a self,
        delivery_id: Uuid,
        fencing_version: i64,
        entry: Option<&'a MoogleMailboxSentEntry>,
        start: Option<MoogleMailboxSweepPosition>,
        sweep_id: Uuid,
        oldest_visible: Timestamp,
    ) -> ApplicationFuture<'a, bool> {
        Box::pin(async move {
            if delivery_id.is_nil()
                || fencing_version <= 0
                || sweep_id.is_nil()
                || entry.is_some_and(|entry| {
                    !auction_domain::valid_positive_decimal(&entry.external_mail_id, 64)
                })
            {
                return Err(storage_error("MoogleMail 失败投递详情轮转参数无效"));
            }
            failed_sent::begin_detail_from(
                self.pool(),
                delivery_id,
                fencing_version,
                entry,
                start,
                Some(sweep_id),
                Some(oldest_visible),
            )
            .await
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
            writes::begin_compose_discard(self.pool(), delivery_id, fencing_version, expectation)
                .await
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
    ) -> ApplicationFuture<'a, bool> {
        Box::pin(async move {
            if delivery_id.is_nil() || fencing_version <= 0 || !observation.is_object() {
                return Err(storage_error("MoogleMail Sent 观察参数无效"));
            }
            writes::record_observation(
                self.pool(),
                delivery_id,
                fencing_version,
                "mailbox_sweep",
                observation,
                None,
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
    ) -> ApplicationFuture<'a, bool> {
        Box::pin(async move {
            if delivery_id.is_nil()
                || fencing_version <= 0
                || !auction_domain::valid_positive_decimal(external_mail_id, 64)
                || !observation.is_object()
            {
                return Err(storage_error("MoogleMail Sent 目击参数无效"));
            }
            writes::record_observation(
                self.pool(),
                delivery_id,
                fencing_version,
                "sent_reference_sighting",
                observation,
                Some(external_mail_id),
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
            writes::record_observation(
                self.pool(),
                delivery_id,
                fencing_version,
                "compose_discard",
                observation,
                None,
            )
            .await
            .map(|_| ())
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
            writes::bind_recovered_send(
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
            writes::bind_recovered_action(
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

    fn record_return_take_observation<'a>(
        &'a self,
        delivery_id: Uuid,
        fencing_version: i64,
        reason: SettlementReturnTakeUnknownReason,
        sweep_retries: i64,
        expectation: &'a DeliveryTakeExpectation,
        observation: &'a DeliverySendCredentialBinding,
    ) -> ApplicationFuture<'a, bool> {
        Box::pin(async move {
            if !observation::valid_observation(
                delivery_id,
                fencing_version,
                reason,
                sweep_retries,
                expectation,
            ) {
                return Err(storage_error("结算退件 Take 观察参数无效"));
            }
            let observed = crate::database::settlement::returns::TakeObservation {
                take_delivery_id: delivery_id,
                fencing_version,
                reason,
                sweep_retries,
                expectation,
            };
            observation::record_return_take_observation(self.pool(), &observed, observation).await
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
            writes::requeue_unproven(
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
