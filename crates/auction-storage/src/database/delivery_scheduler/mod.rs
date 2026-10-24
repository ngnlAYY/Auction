//! PostgreSQL delivery outbox 的 claim、发送中 fencing 和终态写入适配器；传输结果只在
//! claim 的 token/fencing 仍有效时写回，远端结果无法确定时必须置为 `manual_reconcile`。

use auction_application::{
    ApplicationError, DeliveryClaimExclusions, DeliverySendResult, DeliveryTransportError,
    DeliveryWorkClaim, DeliveryWorkStore,
};
use chrono::{DateTime, Utc};

#[cfg(feature = "postgres")]
use auction_application::RequestHash;
#[cfg(feature = "postgres")]
use uuid::Uuid;

use super::DatabaseStore;

/// 邮件资产处置的发送资格同时供领取、发送前 fence 和历史隔离扫描使用。
#[cfg(feature = "postgres")]
macro_rules! delivery_mail_disposition_gate {
    () => {
        r#"(
    deliveries.origin_kind <> 'mail_intake'
    OR deliveries.payload ->> 'operation' = 'send'
    OR (
        deliveries.payload ->> 'operation' IN ('take', 'return')
        AND NOT EXISTS (
            SELECT 1 FROM public.deliveries AS opposite
            WHERE opposite.origin_kind = 'mail_intake'
              AND opposite.payload ->> 'world' = deliveries.payload ->> 'world'
              AND opposite.payload ->> 'external_mail_id' = deliveries.payload ->> 'external_mail_id'
              AND opposite.payload ->> 'operation' = CASE
                  WHEN deliveries.payload ->> 'operation' = 'take' THEN 'return'
                  ELSE 'take'
              END
        )
        AND ((deliveries.payload ->> 'operation' = 'take') = EXISTS (
            SELECT 1 FROM public.auction_submissions AS submission
            INNER JOIN public.auctions AS auction ON auction.id = submission.auction_id
            WHERE submission.source_kind = 'mail'
              AND auction.world_id = deliveries.payload ->> 'world'
              AND submission.external_mail_id = deliveries.payload ->> 'external_mail_id'
        ))
    )
)"#
    };
}

/// 普通 MailIntake delivery 必须等待入站邮件及其命令账本完成，并由决定元数据明确授权当前
/// delivery；论坛通知沿用 MailIntake origin，但通过论坛命令账本证明真实 owner。
/// 已录入拍卖的邮件不能退回，相反资产意图也不能同时发送；该门禁覆盖升级前已 pending/claimed 的
/// 冲突记录，保留原账本供人工核对，不能靠重放命令重新获得发送授权。
#[cfg(feature = "postgres")]
macro_rules! delivery_completion_gate {
    () => {
        concat!(
            r#"(
    deliveries.origin_kind <> 'mail_intake'
    OR (
    (
        deliveries.origin_kind = 'mail_intake'
        AND deliveries.payload ->> 'template' = 'forum_auction_notice'
        AND deliveries.payload ->> 'operation' = 'send'
        AND EXISTS (
            SELECT 1
            FROM public.auction_forum_bid_commands AS forum_command
            WHERE forum_command.command_id = deliveries.command_id
              AND forum_command.command_kind = 'notify'
              AND forum_command.world_id = deliveries.payload ->> 'world'
        )
    )
    OR EXISTS (
        SELECT 1
        FROM public.inbound_mail AS inbound
        INNER JOIN public.mail_commands AS command ON command.mail_id = inbound.id
        WHERE inbound.world_id = deliveries.payload ->> 'world'
          AND inbound.external_mail_id = deliveries.payload ->> 'external_mail_id'
          AND inbound.status IN ('accepted', 'rejected', 'failed')
          AND inbound.decision_code <> 'parsed_pending_processing'
          AND jsonb_typeof(inbound.decision_metadata -> 'delivery_ids') = 'array'
          AND inbound.decision_metadata -> 'delivery_ids' ? (deliveries.id::text)
          AND octet_length(inbound.content_digest) = 32
          AND inbound.content_digest <> decode(repeat('00', 32), 'hex')
          AND command.mail_id = inbound.id
          AND command.request_digest = inbound.content_digest
          AND command.result_payload = jsonb_build_object(
              'mail_id', inbound.id,
              'status', inbound.status,
              'decision_code', inbound.decision_code
          )
) )
)"#,
            " AND ",
            delivery_mail_disposition_gate!(),
            " AND ",
            crate::database::lot_watch::delivery_watch_gate!()
        )
    };
}

#[cfg(feature = "postgres")]
pub(super) mod attempts;
#[cfg(feature = "postgres")]
mod cadence;
mod capability;
#[cfg(feature = "postgres")]
mod cash;
#[cfg(feature = "postgres")]
mod claims;
#[cfg(feature = "postgres")]
mod expired;
mod mapping;
#[cfg(feature = "postgres")]
mod outcome;
#[cfg(feature = "postgres")]
mod quarantine;
#[cfg(feature = "postgres")]
mod state;
#[cfg(feature = "postgres")]
pub(super) mod take_attempts;
#[cfg(feature = "postgres")]
mod wakeup;
#[cfg(feature = "postgres")]
mod watch;

#[cfg(feature = "postgres")]
use super::delivery_audit::{
    CLAIM, DeliveryAudit, FAIL, OUTCOME_UNKNOWN, RECONCILE_EXPIRED_SENDING, RELEASE, START_SENDING,
    SUCCEED, append as append_delivery_audit, worker_state_metadata,
};
#[cfg(feature = "postgres")]
use claims::*;
use mapping::*;
#[cfg(feature = "postgres")]
use outcome::*;
#[cfg(feature = "postgres")]
use state::*;

#[cfg(feature = "postgres")]
use sqlx::{Postgres, Row, postgres::PgRow};

#[cfg(feature = "postgres")]
const EXPIRED_SENDING_RECONCILE_LIMIT: i64 = 100;

impl DeliveryWorkStore for DatabaseStore {
    fn subscribe_wakeups(
        &self,
    ) -> auction_application::ApplicationFuture<'_, Box<dyn auction_application::DeliveryWakeup>>
    {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => wakeup::subscribe(store.raw_pool()).await,
            }
        })
    }

    fn claim_due(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        sending_grace: std::time::Duration,
        exclusions: DeliveryClaimExclusions,
        limit: u16,
    ) -> auction_application::ApplicationFuture<'_, Vec<DeliveryWorkClaim>> {
        Box::pin(async move {
            if worker_id.is_empty()
                || worker_id != worker_id.trim()
                || limit == 0
                || lease_until <= now
                || sending_grace.is_zero()
                || sending_grace > std::time::Duration::from_secs(86_400)
                || exclusions.unavailable_worlds.len() > 2
                || exclusions
                    .unavailable_worlds
                    .iter()
                    .any(|world| !matches!(world.as_str(), "main" | "isekai"))
                || exclusions.unavailable_send_worlds.len() > 2
                || exclusions
                    .unavailable_send_worlds
                    .iter()
                    .any(|world| !matches!(world.as_str(), "main" | "isekai"))
                || exclusions.recipients.len() > usize::from(u16::MAX)
                || exclusions.recipients.iter().any(|excluded| {
                    excluded.world.is_empty()
                        || excluded.world != excluded.world.trim()
                        || excluded.world.chars().any(char::is_control)
                        || excluded.recipient.is_empty()
                        || excluded.recipient != excluded.recipient.trim()
                        || excluded.recipient.chars().any(char::is_control)
                })
            {
                return Err(repository_error("delivery 领取参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => claim_postgres(
                    store,
                    &worker_id,
                    now,
                    lease_until,
                    sending_grace,
                    &exclusions,
                    limit,
                )
                .await
                .map_err(ApplicationError::from),
            }
        })
    }

    fn start_sending(
        &self,
        _claim: DeliveryWorkClaim,
        _started_at: DateTime<Utc>,
    ) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => start_sending_postgres(
                    store.pool(),
                    &_claim,
                    _started_at,
                    &store.delivery_send_intervals,
                )
                .await
                .map_err(ApplicationError::from),
            }
        })
    }

    fn start_sending_authorized<'a>(
        &'a self,
        _claim: DeliveryWorkClaim,
        _started_at: DateTime<Utc>,
        _evidence: Option<&'a auction_application::SettlementInternalEvidenceSnapshot>,
        _expectation: Option<&'a auction_application::DeliveryPreparedExpectation>,
    ) -> auction_application::ApplicationFuture<'a, auction_application::DeliveryFenceOutcome> {
        Box::pin(async move {
            // 旧入口没有原单调截止，不能将有证据的现金授权降级为无期限提交。
            if _evidence.is_some() {
                return Err(ApplicationError::CapabilityUnavailable(
                    "deadline-bound delivery cash authorization",
                ));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    cash::run(
                        store.pool(),
                        &_claim,
                        _started_at,
                        None,
                        None,
                        &store.delivery_send_intervals,
                        _expectation,
                    )
                    .await
                }
            }
        })
    }

    fn start_sending_authorized_until<'a>(
        &'a self,
        _claim: DeliveryWorkClaim,
        _started_at: DateTime<Utc>,
        _evidence: &'a auction_application::SettlementInternalEvidenceSnapshot,
        _deadline: std::time::Instant,
        _expectation: Option<&'a auction_application::DeliveryPreparedExpectation>,
    ) -> auction_application::ApplicationFuture<'a, auction_application::DeliveryFenceOutcome> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    cash::run(
                        store.pool(),
                        &_claim,
                        _started_at,
                        Some(_evidence),
                        Some(_deadline),
                        &store.delivery_send_intervals,
                        _expectation,
                    )
                    .await
                }
            }
        })
    }

    fn reject_preparation(
        &self,
        _claim: DeliveryWorkClaim,
        _completed_at: DateTime<Utc>,
    ) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    reject_preparation_postgres(store.pool(), &_claim, _completed_at)
                        .await
                        .map_err(ApplicationError::from)
                }
            }
        })
    }

    fn release(
        &self,
        _claim: DeliveryWorkClaim,
        _released_at: DateTime<Utc>,
    ) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => release_postgres(store.pool(), &_claim, _released_at)
                    .await
                    .map_err(ApplicationError::from),
            }
        })
    }

    fn renew(
        &self,
        _claim: DeliveryWorkClaim,
        _renewed_at: DateTime<Utc>,
        _lease_until: DateTime<Utc>,
    ) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if _lease_until <= _renewed_at {
                return Err(repository_error("delivery 续租时间无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    renew_postgres(store.pool(), &_claim, _renewed_at, _lease_until)
                        .await
                        .map_err(ApplicationError::from)
                }
            }
        })
    }

    fn succeed(
        &self,
        _claim: DeliveryWorkClaim,
        result: DeliverySendResult,
        _completed_at: DateTime<Utc>,
    ) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if result.external_reference.is_empty()
                || result.external_reference != result.external_reference.trim()
                || !result.result_metadata.is_object()
            {
                return Err(repository_error("delivery 成功结果无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    succeed_postgres(store.pool(), &_claim, &result, _completed_at)
                        .await
                        .map_err(ApplicationError::from)
                }
            }
        })
    }

    fn fail(
        &self,
        _claim: DeliveryWorkClaim,
        error: DeliveryTransportError,
        _completed_at: DateTime<Utc>,
    ) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            let failure_code = bounded_text(error.failure_code, 64);
            let safe_detail = bounded_text(error.safe_detail, 512);
            let external_reference = error.external_reference.map(|value| bounded_text(value, 512));
            let mut result_metadata = error.result_metadata;
            if failure_code.is_empty()
                || failure_code != failure_code.trim()
                || safe_detail.is_empty()
                || safe_detail != safe_detail.trim()
                || external_reference.as_ref().is_some_and(|value| {
                    value.is_empty() || value != value.trim() || value.chars().any(char::is_control)
                })
                || !result_metadata.is_object()
            {
                return Err(repository_error("delivery 失败结果无效"));
            }
            let Some(metadata) = result_metadata.as_object_mut() else {
                return Err(repository_error("delivery 失败 metadata 必须是 object"));
            };
            metadata.insert("safe_detail".to_owned(), safe_detail.clone().into());
            metadata.insert("outcome_unknown".to_owned(), error.outcome_unknown.into());
            if serde_json::to_vec(&result_metadata)
                .map_err(|_| repository_error("delivery 失败 metadata 无法序列化"))?
                .len()
                > 16 * 1024
            {
                return Err(repository_error("delivery 失败 metadata 超出边界"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => fail_postgres(
                    store.pool(),
                    &_claim,
                    &failure_code,
                    error.outcome_unknown,
                    external_reference.as_deref(),
                    &result_metadata,
                    _completed_at,
                )
                .await
                .map_err(ApplicationError::from),
            }
        })
    }
}
