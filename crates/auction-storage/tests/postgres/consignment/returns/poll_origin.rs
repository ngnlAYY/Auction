//! AcquireReturn 的生产路径（PIWS3b）：poll 预读按系统 RTS 线索定位寄售原轮，规划交退件接管，同事务决定
//! 落成 `return_acquired`；伪造的来源只用来定位，写端锁内复判不成立仍整封隔离。
//!
//! 只测核心规则：安全边界（来源失败关闭、零接管行）与资产外写（只有可信 RTS 才建唯一 Take 工作）。

use std::sync::Arc;

use auction_application::consignment::{
    ConsignmentRtsLocator, PollEnvelopeStore, PollEnvelopeWrite, PollRawObservation,
    ReceiptDisposition,
};
use auction_application::mail::{
    ConsignmentMailInput, ConsignmentMailPlan, ConsignmentMailService, MailMessage, MailPollClaim,
};
use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::deposit::{
    EnvelopeObservation, LegacyReturnEvidence, SenderIdentity, TrustedConsignmentRts,
};
use auction_storage::DatabaseStore;
use auction_time::{Duration, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::OBLIGATION_3;
use super::super::deposit::support::{SESSION, succeeds};
use super::super::ledger136::fixture::REFUND_ROUND;
use super::fixture::{MANIFEST, Rts, seed_main_readiness, seed_refund_rts};
use crate::support::TestDatabase;

fn subject() -> String {
    format!("[RTS] Consignment buyer refund [{OBLIGATION_3}]")
}

/// 退款腿原轮的系统 RTS 观察：Credits、COD 与无装备都和首派冻结身份一致。
fn observation() -> EnvelopeObservation {
    EnvelopeObservation {
        observation_id: Uuid::new_v4(),
        session_binding_digest: [0x33; 32],
        manifest_digest: MANIFEST,
        parser_version: "fake-rts-parser".to_owned(),
        subject_is_consign: false,
        sender_identity: SenderIdentity::Verified,
        list_detail_consistent: true,
        asset_count: Some(0),
        duplicate_raw_eid: false,
        canonical_alias_ambiguous: false,
        unknown_attachment: false,
        credits: Some(0),
        cod: Some(0),
        returnable: true,
        takeable: Some(true),
        system_rts: true,
        legacy_auction_owner: None,
        items: Vec::new(),
    }
}

fn message(mailbox: &str, mid: &str) -> MailMessage {
    MailMessage {
        mailbox_key: mailbox.to_owned(),
        external_mail_id: mid.to_owned(),
        world_id: "main".to_owned(),
        received_at: Timestamp::now(),
        sender: "MoogleMail".to_owned(),
        subject: subject(),
        body: "fake returned body".to_owned(),
        credits: 0,
        cod: 0,
        attachments: Vec::new(),
        returnable: true,
        claimable: true,
        content_digest: [0x5a; 32],
    }
}

/// 邮箱的轮询租约与待读候选；游标停在 "10"。
async fn seed_mailbox(pool: &PgPool, mailbox: &str, mid: &str, token: Uuid) -> MailPollClaim {
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.mail_poll_state
                (mailbox_key, cursor_value, last_poll_at, next_poll_at, consecutive_failures,
                 paused, state_version, updated_at, claim_token, claim_owner, claim_expires_at)
             VALUES ('{mailbox}', '10', clock_timestamp(), clock_timestamp(), 0, false, 1,
                     clock_timestamp(), '{token}', 'fake-poll-worker',
                     clock_timestamp() + interval '1 hour');
             INSERT INTO public.mail_poll_pending_reads
                (mailbox_key, world_id, external_mail_id, sender, subject, received_at)
             VALUES ('{mailbox}', 'main', '{mid}', 'MoogleMail', 'fake rts', clock_timestamp());"
        ),
    )
    .await;
    MailPollClaim {
        mailbox_key: mailbox.to_owned(),
        worker_id: "fake-poll-worker".to_owned(),
        claim_token: token,
        cursor_value: Some("10".to_owned()),
        state_version: 1,
        paused: false,
        attempt_count: 1,
        lease_until: Timestamp::now() + Duration::hours(1),
    }
}

/// 经应用层走一封 RTS：预读（带线索）→ 规划（`origin` 为空时取预读定位的来源）→ poll 同事务决定。
async fn poll_rts(
    store: &DatabaseStore,
    pool: &PgPool,
    mailbox: &str,
    rts: Rts,
    forged: Option<TrustedConsignmentRts>,
) -> (Option<TrustedConsignmentRts>, auction_application::consignment::EnvelopeReceipt) {
    let key: OriginalMailKey = rts.key();
    let mid = rts.mid();
    let claim = seed_mailbox(pool, mailbox, &mid, Uuid::new_v4()).await;
    let observation = observation();
    let preflight = store
        .load_envelope_preflight(
            key.clone(),
            observation.clone(),
            Some("MoogleMail".to_owned()),
            Some(ConsignmentRtsLocator {
                obligation_id: Uuid::parse_str(OBLIGATION_3).expect("义务"),
                dispatch_reference: Some(REFUND_ROUND.to_owned()),
            }),
        )
        .await
        .expect("预读");
    let located = preflight.consignment_rts;
    let message = message(mailbox, &mid);
    let plan = ConsignmentMailService::planner()
        .plan_with_origin(
            ConsignmentMailInput {
                message: &message,
                mail_key: Some(key.clone()),
                observation: observation.clone(),
                reservations: preflight.reservations,
                envelope_seller_user_id: preflight.seller_user_id,
                capabilities: preflight.capabilities,
                read_failure: false,
                existing_decision: preflight.existing_decision,
                now: preflight.now,
            },
            forged.or(located),
        )
        .await
        .expect("规划");
    let ConsignmentMailPlan::First(command) = plan else { panic!("应首次决定：{plan:?}") };
    let now = Timestamp::now();
    let write = PollEnvelopeWrite {
        mail_key: key.clone(),
        command_id: command.command_id,
        command: Some(*command),
        raw: Some(PollRawObservation {
            mail_key: key,
            session_id: Uuid::parse_str(SESSION).expect("会话"),
            mailbox_key: mailbox.to_owned(),
            remote_observation_id: format!("obs-{mid}"),
            observed_at: now,
            parser_version: observation.parser_version.clone(),
            evidence_ref: None,
            subject_digest: [0x11; 32],
            body_digest: [0x22; 32],
            sender: Some("MoogleMail".to_owned()),
            recipient: Some("FakeBot".to_owned()),
            raw_count: Some(0),
            observation,
            legacy_evidence: LegacyReturnEvidence {
                dispatch_reference: Some(REFUND_ROUND.to_owned()),
                ..LegacyReturnEvidence::default()
            },
            received_minute_raw: "2026-11-01T05:00".to_owned(),
            received_timezone: "UTC".to_owned(),
            first_seen_at: now,
            season_window_id: None,
        }),
        claim,
        polled_at: now,
        next_poll_at: now + Duration::hours(1),
        cursor_mid: mid,
    };
    (located, store.decide_envelope_in_poll(write).await.expect("poll 同事务决定"))
}

/// (接管行数, 接管建的 Take 工作数)
async fn intakes(pool: &PgPool) -> (i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.consignment_return_intakes),
                (SELECT count(*) FROM public.mail_disposition_work AS w
                 JOIN public.consignment_return_intakes AS i ON i.take_work_id = w.id)",
    )
    .fetch_one(pool)
    .await
    .expect("读取接管计数")
}

/// 正例：可信寄售 RTS 经预读定位、规划与 poll 同事务决定落成 `return_acquired`，恰一个接管与 Take 工作；
/// 反例：伪造来源（指向不存在的原轮）照样生成接管命令，但写端锁内复判不成立，整封隔离、零接管行。
#[tokio::test]
async fn postgres_consignment_returns_poll_acquires_trusted_rts_and_quarantines_forged_origin() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let store = Arc::new(database.store().await.expect("运行账号"));
    let pool = owner.postgres_pool();
    seed_refund_rts(pool, &store, Rts { n: 1 }).await;
    seed_main_readiness(pool).await;
    assert_eq!(intakes(pool).await, (0, 0));

    let obligation = Uuid::parse_str(OBLIGATION_3).expect("义务");
    let (located, receipt) = poll_rts(&store, pool, "fake-mailbox-a", Rts { n: 2 }, None).await;
    assert_eq!(
        located,
        Some(TrustedConsignmentRts { obligation_id: obligation, original_round: 1 })
    );
    assert_eq!(receipt.disposition, ReceiptDisposition::ReturnAcquired);
    assert!(receipt.return_intake.is_some());
    assert_eq!(intakes(pool).await, (1, 1));

    let forged = TrustedConsignmentRts { obligation_id: obligation, original_round: 2 };
    let (_, receipt) = poll_rts(&store, pool, "fake-mailbox-b", Rts { n: 3 }, Some(forged)).await;
    assert_eq!(receipt.disposition, ReceiptDisposition::Quarantined);
    assert!(receipt.return_intake.is_none());
    assert_eq!(intakes(pool).await, (1, 1), "伪造来源零接管、零 Take 工作");
}
