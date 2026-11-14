//! PostgreSQL delivery outbox 的 claim、发送中 fencing 和终态写入适配器；传输结果只在
//! claim 的 token/fencing 仍有效时写回，远端结果无法确定时必须置为 `manual_reconcile`。

use auction_application::core::ApplicationError;
use auction_application::delivery::{
    DeliveryClaimExclusions, DeliverySendResult, DeliveryTransportError, DeliveryWorkClaim,
    DeliveryWorkStore,
};
use chrono::{DateTime, Utc};

use crate::database::DatabaseStore;

/// 邮件资产处置的发送资格同时供领取、发送前 fence 和历史隔离扫描使用。
macro_rules! delivery_mail_disposition_gate {
    () => {
        r#"(
    deliveries.origin_kind <> 'mail_intake'
    OR deliveries.payload_operation = 'send'
    OR (
        deliveries.payload_operation IN ('take', 'return')
        AND NOT EXISTS (
            SELECT 1 FROM public.deliveries AS opposite
            WHERE opposite.origin_kind = 'mail_intake'
              AND opposite.payload_world = deliveries.payload_world
              AND opposite.payload ->> 'external_mail_id' = deliveries.payload ->> 'external_mail_id'
              AND opposite.payload_operation = CASE
                  WHEN deliveries.payload_operation = 'take' THEN 'return'
                  ELSE 'take'
              END
        )
        AND ((deliveries.payload_operation = 'take') = EXISTS (
            SELECT 1 FROM public.auction_submissions AS submission
            INNER JOIN public.auctions AS auction ON auction.id = submission.auction_id
            WHERE submission.source_kind = 'mail'
              AND auction.world_id = deliveries.payload_world
              AND submission.external_mail_id = deliveries.payload ->> 'external_mail_id'
        ))
    )
)"#
    };
}

/// 换季提醒是否仍有未完成义务：扫描、预检退役与发送围栏共用同一口径。
/// 未经恢复显式激活的 contingent 退款不是当前欠付，既不能作为提醒理由，也不能阻挡退役。
macro_rules! season_reminder_outstanding_gate {
    () => {
        r#"EXISTS (
    SELECT 1
    FROM public.settlement_obligations AS obligation
    JOIN public.auctions AS auction ON auction.id = obligation.auction_id
    JOIN public.settlement_obligation_fulfillments AS fulfillment
      ON fulfillment.obligation_id = obligation.id
    LEFT JOIN public.users AS recipient ON recipient.id = obligation.recipient_user_id
    WHERE auction.world_id = 'isekai'
      AND fulfillment.execution_state IN ('pending', 'prepared', 'enqueued')
      AND obligation.business_state <> 'contingent'
      AND COALESCE(obligation.recipient_game_identity, recipient.username) = deliveries.recipient
)"#
    };
}

/// 通知类预检资格在领取与发送围栏都必须复核：批量退役清理有行数上限，
/// 不能让领取早于边界/开拍的通知在清理之后再进入 sending。
///
/// 换季提醒：冻结边界已过或已无未完成义务时不得发送；边界只从冻结载荷读取。
/// 投稿接收回执：原场次已开拍或撤场后不得补寄。
macro_rules! delivery_notice_eligibility_gate {
    () => {
        concat!(
            r#"(
    deliveries.origin_kind <> 'season_reminder'
    OR (
        CASE
            WHEN deliveries.payload ->> 'external_mail_id'
                 ~ '^season-end:[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$'
            THEN (substring(deliveries.payload ->> 'external_mail_id' from 12))::timestamptz
                 > clock_timestamp()
            ELSE false
        END
        AND "#,
            season_reminder_outstanding_gate!(),
            r#"
    )
)
AND (
    deliveries.origin_kind <> 'mail_intake'
    OR COALESCE(deliveries.payload_operation, '') <> 'send'
    OR COALESCE(deliveries.payload_template, '') <> 'auction_submission_receipt'
    OR NOT EXISTS (
        SELECT 1
        FROM public.auction_submissions AS submission
        JOIN public.auctions AS auction ON auction.id = submission.auction_id
        WHERE submission.source_kind = 'mail'
          AND submission.external_mail_id = deliveries.payload ->> 'external_mail_id'
          AND auction.world_id = deliveries.payload_world
          AND auction.status NOT IN ('draft', 'preview')
    )
)
AND (
    -- 玩家通知收件人改名后，冻结旧名不得再进入 sending；领取时的退役清理有行数上限，
    -- 必须在发送围栏复核同一谓词（当前名不存在且存在未撤回改名事实）。
    deliveries.origin_kind NOT IN ('seller_summary', 'season_reminder')
    OR EXISTS (
        SELECT 1 FROM public.users AS current
        WHERE current.username = deliveries.recipient
    )
    OR NOT EXISTS (
        SELECT 1 FROM public.user_renames AS rename
        WHERE rename.previous_username = deliveries.recipient
          AND rename.reverted_at IS NULL
    )
)"#
        )
    };
}

/// 普通 MailIntake delivery 必须等待入站邮件及其命令账本完成，并由决定元数据明确授权当前
/// delivery；论坛通知沿用 MailIntake origin，但通过论坛命令账本证明真实 owner。
/// 已录入拍卖的邮件不能退回，相反资产意图也不能同时发送；该门禁覆盖升级前已 pending/claimed 的
/// 冲突记录，保留原账本供人工核对，不能靠重放命令重新获得发送授权。
/// 通知类资格（换季提醒边界/未完成义务、投稿回执原场次状态）同样在此复核，领取与发送共用。
macro_rules! delivery_completion_gate {
    () => {
        concat!(
            r#"(
    deliveries.origin_kind <> 'mail_intake'
    OR (
    (
        deliveries.origin_kind = 'mail_intake'
        AND deliveries.payload_template = 'forum_auction_notice'
        AND deliveries.payload_operation = 'send'
        AND EXISTS (
            SELECT 1
            FROM public.auction_forum_bid_commands AS forum_command
            WHERE forum_command.command_id = deliveries.command_id
              AND forum_command.command_kind = 'notify'
              AND forum_command.world_id = deliveries.payload_world
        )
    )
    OR EXISTS (
        SELECT 1
        FROM public.inbound_mail AS inbound
        INNER JOIN public.mail_commands AS command ON command.mail_id = inbound.id
        WHERE inbound.world_id = deliveries.payload_world
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
            crate::database::lot_watch::delivery_watch_gate!(),
            " AND ",
            delivery_notice_eligibility_gate!()
        )
    };
}

pub(in crate::database) mod attempts;
mod cadence;
mod capability;
mod cash;
mod claims;
mod expired;
mod mapping;
mod outcome;
mod quarantine;
mod state;
pub(in crate::database) mod take_attempts;
mod wakeup;
mod watch;
pub(crate) use wakeup::publish_after_commit;

use self::claims::{begin_claim_cycle_postgres, claim_postgres};
use self::mapping::{bounded_text, storage_error};
use self::outcome::{fail_postgres, reject_preparation_postgres, succeed_postgres};
use self::state::{release_postgres, renew_postgres, start_sending_postgres};
use crate::database::delivery::audit::{
    DeliveryAudit, START_SENDING, append as append_delivery_audit, worker_state_metadata,
};

const EXPIRED_SENDING_RECONCILE_LIMIT: i64 = 100;

impl DeliveryWorkStore for DatabaseStore {
    fn subscribe_wakeups(
        &self,
    ) -> auction_application::auction::ApplicationFuture<
        '_,
        Box<dyn auction_application::delivery::DeliveryWakeup>,
    > {
        Box::pin(async move { wakeup::subscribe(self.pool()).await })
    }

    fn begin_claim_cycle(
        &self,
        now: DateTime<Utc>,
        sending_grace: std::time::Duration,
    ) -> auction_application::auction::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            begin_claim_cycle_postgres(self, now, sending_grace)
                .await
                .map_err(auction_application::core::ApplicationError::from)
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
    ) -> auction_application::auction::ApplicationFuture<'_, Vec<DeliveryWorkClaim>> {
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
                return Err(storage_error("delivery 领取参数无效"));
            }
            claim_postgres(self, &worker_id, now, lease_until, &exclusions, limit)
                .await
                .map_err(ApplicationError::from)
        })
    }

    fn start_sending(
        &self,
        claim: DeliveryWorkClaim,
        started_at: DateTime<Utc>,
    ) -> auction_application::auction::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            let intervals = self.delivery_send_intervals();
            start_sending_postgres(self.pool(), &claim, started_at, &intervals)
                .await
                .map_err(ApplicationError::from)
        })
    }

    fn start_sending_authorized<'a>(
        &'a self,
        claim: DeliveryWorkClaim,
        started_at: DateTime<Utc>,
        evidence: Option<&'a auction_application::settlement::SettlementInternalEvidenceSnapshot>,
        expectation: Option<&'a auction_application::delivery::DeliveryPreparedExpectation>,
    ) -> auction_application::auction::ApplicationFuture<
        'a,
        auction_application::delivery::DeliveryFenceOutcome,
    > {
        Box::pin(async move {
            // 旧入口没有原单调截止，不能将有证据的现金授权降级为无期限提交。
            if evidence.is_some() {
                return Err(ApplicationError::CapabilityUnavailable(
                    "deadline-bound delivery cash authorization",
                ));
            }
            let intervals = self.delivery_send_intervals();
            cash::run(self.pool(), &claim, started_at, None, None, &intervals, expectation).await
        })
    }

    fn start_sending_authorized_until<'a>(
        &'a self,
        claim: DeliveryWorkClaim,
        started_at: DateTime<Utc>,
        evidence: &'a auction_application::settlement::SettlementInternalEvidenceSnapshot,
        deadline: std::time::Instant,
        expectation: Option<&'a auction_application::delivery::DeliveryPreparedExpectation>,
    ) -> auction_application::auction::ApplicationFuture<
        'a,
        auction_application::delivery::DeliveryFenceOutcome,
    > {
        Box::pin(async move {
            let intervals = self.delivery_send_intervals();
            cash::run(
                self.pool(),
                &claim,
                started_at,
                Some(evidence),
                Some(deadline),
                &intervals,
                expectation,
            )
            .await
        })
    }

    fn reject_preparation(
        &self,
        claim: DeliveryWorkClaim,
        completed_at: DateTime<Utc>,
    ) -> auction_application::auction::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            reject_preparation_postgres(self.pool(), &claim, completed_at)
                .await
                .map_err(ApplicationError::from)
        })
    }

    fn release(
        &self,
        claim: DeliveryWorkClaim,
        released_at: DateTime<Utc>,
    ) -> auction_application::auction::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            release_postgres(self.pool(), &claim, released_at).await.map_err(ApplicationError::from)
        })
    }

    fn renew(
        &self,
        claim: DeliveryWorkClaim,
        renewed_at: DateTime<Utc>,
        lease_until: DateTime<Utc>,
    ) -> auction_application::auction::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if lease_until <= renewed_at {
                return Err(storage_error("delivery 续租时间无效"));
            }
            renew_postgres(self.pool(), &claim, renewed_at, lease_until)
                .await
                .map_err(ApplicationError::from)
        })
    }

    fn succeed(
        &self,
        claim: DeliveryWorkClaim,
        result: DeliverySendResult,
        completed_at: DateTime<Utc>,
    ) -> auction_application::auction::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if result.external_reference.is_empty()
                || result.external_reference != result.external_reference.trim()
                || !result.result_metadata.is_object()
            {
                return Err(storage_error("delivery 成功结果无效"));
            }
            succeed_postgres(self.pool(), &claim, &result, completed_at)
                .await
                .map_err(ApplicationError::from)
        })
    }

    fn fail(
        &self,
        claim: DeliveryWorkClaim,
        error: DeliveryTransportError,
        completed_at: DateTime<Utc>,
    ) -> auction_application::auction::ApplicationFuture<'_, ()> {
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
                return Err(storage_error("delivery 失败结果无效"));
            }
            let Some(metadata) = result_metadata.as_object_mut() else {
                return Err(storage_error("delivery 失败 metadata 必须是 object"));
            };
            metadata.insert("safe_detail".to_owned(), safe_detail.clone().into());
            metadata.insert("outcome_unknown".to_owned(), error.outcome_unknown.into());
            if serde_json::to_vec(&result_metadata)
                .map_err(|_| storage_error("delivery 失败 metadata 无法序列化"))?
                .len()
                > 16 * 1024
            {
                return Err(storage_error("delivery 失败 metadata 超出边界"));
            }
            fail_postgres(
                self.pool(),
                &claim,
                &failure_code,
                error.outcome_unknown,
                external_reference.as_deref(),
                &result_metadata,
                completed_at,
            )
            .await
            .map_err(ApplicationError::from)
        })
    }
}

/// 通知类预检资格必须由领取与发送共用的门禁复核，而不是只靠有行数上限的退役清理。
/// 这里直接执行该门禁：换季提醒的冻结边界/未完成义务与投稿回执的原场次状态都按
/// 数据库此刻的状态判定，contingent 退款不算未完成义务。
#[cfg(test)]
mod notice_gate_tests {
    use sqlx::{Connection, Executor, Row};

    /// 门禁引用的全部来源表；只用最小列集合，行数据由各用例显式插入。
    const FIXTURE: &str = r#"
        CREATE TEMP TABLE deliveries (
            id uuid, command_id uuid, request_digest bytea, origin_kind text, origin_id text,
            recipient text, payload jsonb, payload_world text, payload_operation text,
            payload_template text
        );
        CREATE TEMP TABLE auctions (id uuid, world_id text, status text);
        CREATE TEMP TABLE auction_submissions (
            id uuid, source_kind text, external_mail_id text, auction_id uuid
        );
        CREATE TEMP TABLE auction_forum_bid_commands (
            command_id uuid, command_kind text, world_id text
        );
        CREATE TEMP TABLE inbound_mail (
            id uuid, world_id text, external_mail_id text, status text, decision_code text,
            decision_metadata jsonb, content_digest bytea
        );
        CREATE TEMP TABLE mail_commands (mail_id uuid, request_digest bytea, result_payload jsonb);
        CREATE TEMP TABLE lot_watch_notifications (
            delivery_id uuid, channel text, command_id uuid, request_digest bytea, recipient text,
            payload jsonb, invalidated_at timestamptz, user_id uuid, lot_id uuid,
            watch_version bigint
        );
        CREATE TEMP TABLE lot_watches (
            user_id uuid, lot_id uuid, watched boolean, channel text, version bigint
        );
        CREATE TEMP TABLE lots (id uuid, leader_user_id uuid, effective_end_at timestamptz);
        CREATE TEMP TABLE settlement_obligations (
            id uuid, auction_id uuid, business_state text, recipient_user_id uuid,
            recipient_game_identity text
        );
        CREATE TEMP TABLE settlement_obligation_fulfillments (
            obligation_id uuid, execution_state text
        );
        CREATE TEMP TABLE users (id uuid, username text);
        CREATE TEMP TABLE user_renames (previous_username text, reverted_at timestamptz);

        INSERT INTO pg_temp.users VALUES
            ('00000000-0000-0000-0000-0000000000c9', 'alice');
        INSERT INTO pg_temp.user_renames VALUES ('old-name', NULL);

        INSERT INTO pg_temp.auctions VALUES
            ('00000000-0000-0000-0000-0000000000a1', 'isekai', 'preview'),
            ('00000000-0000-0000-0000-0000000000a2', 'isekai', 'live');
        INSERT INTO pg_temp.auction_submissions VALUES
            ('00000000-0000-0000-0000-0000000000b1', 'mail', 'mail-preview',
             '00000000-0000-0000-0000-0000000000a1'),
            ('00000000-0000-0000-0000-0000000000b2', 'mail', 'mail-live',
             '00000000-0000-0000-0000-0000000000a2');

        -- 只保留已激活义务：alice 有 ready 义务，bob 只有未激活的 contingent 退款。
        INSERT INTO pg_temp.settlement_obligations VALUES
            ('00000000-0000-0000-0000-0000000000d1', '00000000-0000-0000-0000-0000000000a1',
             'ready', NULL, 'alice'),
            ('00000000-0000-0000-0000-0000000000d2', '00000000-0000-0000-0000-0000000000a1',
             'contingent', NULL, 'bob');
        INSERT INTO pg_temp.settlement_obligation_fulfillments VALUES
            ('00000000-0000-0000-0000-0000000000d1', 'pending'),
            ('00000000-0000-0000-0000-0000000000d2', 'pending');

        INSERT INTO pg_temp.inbound_mail VALUES
            ('00000000-0000-0000-0000-0000000000c1', 'isekai', 'mail-preview', 'accepted',
             'imported',
             jsonb_build_object('delivery_ids',
                 jsonb_build_array('00000000-0000-0000-0000-0000000000e5')),
             decode(repeat('11', 32), 'hex')),
            ('00000000-0000-0000-0000-0000000000c2', 'isekai', 'mail-live', 'accepted',
             'imported',
             jsonb_build_object('delivery_ids',
                 jsonb_build_array('00000000-0000-0000-0000-0000000000e6')),
             decode(repeat('22', 32), 'hex'));
        INSERT INTO pg_temp.mail_commands VALUES
            ('00000000-0000-0000-0000-0000000000c1', decode(repeat('11', 32), 'hex'),
             jsonb_build_object('mail_id', '00000000-0000-0000-0000-0000000000c1'::uuid,
                                'status', 'accepted', 'decision_code', 'imported')),
            ('00000000-0000-0000-0000-0000000000c2', decode(repeat('22', 32), 'hex'),
             jsonb_build_object('mail_id', '00000000-0000-0000-0000-0000000000c2'::uuid,
                                'status', 'accepted', 'decision_code', 'imported'));

        INSERT INTO pg_temp.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             payload_world, payload_operation, payload_template)
        VALUES
            ('00000000-0000-0000-0000-0000000000e1',
             '00000000-0000-0000-0000-0000000000f1', decode(repeat('44', 32), 'hex'),
             'season_reminder', 'season_reminder:1:alice', 'alice',
             '{"external_mail_id":"season-end:2999-01-01T00:00:00Z","world":"isekai","operation":"send","template":"isekai_season_reminder"}'::jsonb,
             'isekai', 'send', 'isekai_season_reminder'),
            ('00000000-0000-0000-0000-0000000000e2',
             '00000000-0000-0000-0000-0000000000f2', decode(repeat('44', 32), 'hex'),
             'season_reminder', 'season_reminder:1:alice', 'alice',
             '{"external_mail_id":"season-end:2000-01-01T00:00:00Z","world":"isekai","operation":"send","template":"isekai_season_reminder"}'::jsonb,
             'isekai', 'send', 'isekai_season_reminder'),
            ('00000000-0000-0000-0000-0000000000e3',
             '00000000-0000-0000-0000-0000000000f3', decode(repeat('44', 32), 'hex'),
             'season_reminder', 'season_reminder:1:bob', 'bob',
             '{"external_mail_id":"season-end:2999-01-01T00:00:00Z","world":"isekai","operation":"send","template":"isekai_season_reminder"}'::jsonb,
             'isekai', 'send', 'isekai_season_reminder'),
            ('00000000-0000-0000-0000-0000000000e4',
             '00000000-0000-0000-0000-0000000000f4', decode(repeat('44', 32), 'hex'),
             'season_reminder', 'season_reminder:1:carol', 'carol',
             '{"external_mail_id":"season-end:2999-01-01T00:00:00Z","world":"isekai","operation":"send","template":"isekai_season_reminder"}'::jsonb,
             'isekai', 'send', 'isekai_season_reminder'),
            ('00000000-0000-0000-0000-0000000000e5',
             '00000000-0000-0000-0000-0000000000f5', decode(repeat('55', 32), 'hex'),
             'mail_intake', 'mail_intake:mail-preview', 'alice',
             '{"external_mail_id":"mail-preview","world":"isekai","operation":"send","template":"auction_submission_receipt"}'::jsonb,
             'isekai', 'send', 'auction_submission_receipt'),
            ('00000000-0000-0000-0000-0000000000e6',
             '00000000-0000-0000-0000-0000000000f6', decode(repeat('55', 32), 'hex'),
             'mail_intake', 'mail_intake:mail-live', 'alice',
             '{"external_mail_id":"mail-live","world":"isekai","operation":"send","template":"auction_submission_receipt"}'::jsonb,
             'isekai', 'send', 'auction_submission_receipt'),
            ('00000000-0000-0000-0000-0000000000e7',
             '00000000-0000-0000-0000-0000000000f7', decode(repeat('66', 32), 'hex'),
             'seller_summary', 'seller_summary:old-name', 'old-name',
             '{"world":"main","operation":"send","template":"auction_seller_summary"}'::jsonb,
             'main', 'send', 'auction_seller_summary'),
            ('00000000-0000-0000-0000-0000000000e8',
             '00000000-0000-0000-0000-0000000000f8', decode(repeat('66', 32), 'hex'),
             'seller_summary', 'seller_summary:alice', 'alice',
             '{"world":"main","operation":"send","template":"auction_seller_summary"}'::jsonb,
             'main', 'send', 'auction_seller_summary');
    "#;

    #[tokio::test]
    #[ignore = "需要 AUCTION_TEST_DATABASE_URL 指向隔离的 PostgreSQL 18"]
    async fn postgres_notice_gate_blocks_stale_reminders_and_receipts()
    -> Result<(), Box<dyn std::error::Error>> {
        let url = std::env::var("AUCTION_TEST_DATABASE_URL")?;
        let mut connection = sqlx::PgConnection::connect(&url).await?;
        let mut tx = connection.begin().await?;
        tx.execute(FIXTURE).await?;
        let gate = delivery_completion_gate!().replace("public.", "pg_temp.");
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT deliveries.id::text AS id, {gate} AS eligible
             FROM pg_temp.deliveries AS deliveries
             ORDER BY deliveries.id::text"
        )))
        .fetch_all(&mut *tx)
        .await?;
        let eligible = rows.iter().map(|row| row.get::<bool, _>("eligible")).collect::<Vec<_>>();
        assert_eq!(
            eligible,
            // 顺序 e1..e8：可发、边界已过、仅 contingent、无义务、preview 可发、已开拍、
            // 冻结旧名已改名不可发、当前名可发。
            vec![true, false, false, false, true, false, false, true]
        );
        tx.rollback().await?;
        Ok(())
    }
}
