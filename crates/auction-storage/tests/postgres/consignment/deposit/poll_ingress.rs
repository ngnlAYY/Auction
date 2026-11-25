//! 第 4 期 I2b2：旧拍卖直达的 E1 回归与 poll 同事务决定/前缀。
//!
//! 三个用例只用运行账号连接临时 PG18：旧投稿续办与旧退款首寄在没有证据会话、原信键或
//! Bot UID 时仍按原路完成，并断言零寄售写入；寄售候选经 `PollEnvelopeStore` 在同一事务
//! 写决定并推进游标，前缀 CAS 失败时决定与观察整体回滚。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use auction_application::admin::{AdminIntakeError, EquipmentKeySealerPort};
use auction_application::consignment::{
    ConsignmentError, DepositCommandService, EnvelopeReceipt, MailReadinessGate, PollEnvelopeStore,
    PollEnvelopeWrite, PollRawObservation,
};
use auction_application::core::{Clock, RequestHash};
use auction_application::mail::{
    MailAuctionIntakeCommand, MailAuctionIntakeLookup, MailAuctionIntakeService, MailAuctionItem,
    MailPollClaim, mail_auction_aggregate_id,
};
use auction_application::recovery::RecoveryWorkStore;
use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::deposit::{
    EnvelopeCapabilities, EnvelopeItemKind, EnvelopeItemObservation, EnvelopeObservation,
    LegacyReturnEvidence, LockedEnvelopeFacts, SenderIdentity,
};
use auction_domain::{
    CommandId, EquipmentId, EquipmentKey, EquipmentKeyEnvelope, SettlementWorld, UserId,
};
use auction_time::{Duration, Timestamp};
use serde_json::json;
use sqlx::{AssertSqlSafe, PgPool};
use uuid::Uuid;

use crate::consignment::deposit::support::{
    SESSION, USER_1, listing_awaiting_deposit, mail_session, reservation_issued, succeeds,
    users_and_key,
};
use crate::support::TestDatabase;
use crate::support::fixtures;

struct TestSealer;

impl EquipmentKeySealerPort for TestSealer {
    fn seal(
        &self,
        _equipment_id: EquipmentId,
        _key: &EquipmentKey,
    ) -> Result<EquipmentKeyEnvelope, AdminIntakeError> {
        Ok(EquipmentKeyEnvelope::new(
            vec![0xa1; 28],
            vec![0xb2; 12],
            "fake-data-key".into(),
            "ab".repeat(32),
        )
        .expect("固定信封"))
    }
}

struct TestClock(Timestamp);

impl Clock for TestClock {
    fn now(&self) -> Timestamp {
        self.0
    }
}

async fn assert_no_consignment_rows(pool: &PgPool) {
    let count: i64 = sqlx::query_scalar(
        "SELECT
            (SELECT count(*) FROM public.mail_evidence_sessions) +
            (SELECT count(*) FROM public.mail_originals) +
            (SELECT count(*) FROM public.mail_initial_decisions) +
            (SELECT count(*) FROM public.mail_quarantines) +
            (SELECT count(*) FROM public.consignment_commands) +
            (SELECT count(*) FROM public.consignment_deposit_reservations)",
    )
    .fetch_one(pool)
    .await
    .expect("读取寄售表计数");
    assert_eq!(count, 0, "旧拍卖直达必须零寄售写入");
}

async fn seed_draft_target(pool: &PgPool, world_id: &str, sequence: i64) -> Uuid {
    let auction_id = Uuid::new_v4();
    let at = Timestamp::now() - Duration::hours(1);
    let mut transaction = pool.begin().await.expect("开启 draft 目标事务");
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, policy_snapshot)
         VALUES ($1, $2, $3, 'Fake I2b2 entry target', 'draft', $4, $4, $4, $4, $5)",
    )
    .bind(auction_id)
    .bind(world_id)
    .bind(sequence)
    .bind(at)
    .bind(fixtures::policy_snapshot(at + Duration::days(1), 50_000, 30_000))
    .execute(&mut *transaction)
    .await
    .expect("写入 draft 场次");
    sqlx::query(
        "INSERT INTO public.auction_intake_targets (world_id, auction_id, target_status)
         VALUES ($1, $2, 'draft')",
    )
    .bind(world_id)
    .bind(auction_id)
    .execute(&mut *transaction)
    .await
    .expect("写入 draft 收件目标");
    transaction.commit().await.expect("提交 draft 目标");
    auction_id
}

#[tokio::test]
async fn postgres_legacy_submission_resumes_without_bot_uid() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let now = Timestamp::now();
    let seller = "FakeSellerOne";
    fixtures::seed_user(pool, Uuid::new_v4(), seller).await;
    seed_draft_target(pool, "main", 92_001).await;
    let command_id =
        CommandId::from_uuid(Uuid::new_v5(&Uuid::NAMESPACE_URL, b"test/i2b2/legacy-submission"));
    let mailbox_key = "mooglemail:main";
    let external_mail_id = "7009201";
    let received_at = now - Duration::minutes(1);
    let command = MailAuctionIntakeCommand {
        command_id,
        aggregate_id: mail_auction_aggregate_id("main"),
        request_hash: RequestHash::new([21; 32]),
        world_id: "main".to_owned(),
        mailbox_key: mailbox_key.to_owned(),
        external_mail_id: external_mail_id.to_owned(),
        seller: seller.to_owned(),
        received_at,
        content_digest: [21; 32],
        items: vec![MailAuctionItem {
            external_equipment_id: "92001".to_owned(),
            name: "Fake Legacy Gear".to_owned(),
            category: "Weapon".to_owned(),
            level: Some(100),
            attributes: json!({}),
            source_fingerprint: [21; 32],
            collected_at: received_at,
            start_price: 100,
            key: EquipmentKey::new("0123456789").expect("固定 key"),
        }],
        now,
    };
    let service = MailAuctionIntakeService::new(
        Arc::new(store.clone()),
        Arc::new(TestSealer),
        Arc::new(TestClock(now)),
    );
    let accepted = service.ingest_mail(command).await.expect("旧投稿必须照原路接受");
    let resumed = service
        .find_accepted(MailAuctionIntakeLookup {
            command_id,
            world_id: "main".to_owned(),
            mailbox_key: mailbox_key.to_owned(),
            external_mail_id: external_mail_id.to_owned(),
            seller: seller.to_owned(),
            received_at,
        })
        .await
        .expect("旧投稿续办查询")
        .expect("没有 Bot UID 也必须恢复原接受事实");
    assert_eq!(resumed.result.auction_id, accepted.auction_id);
    assert_eq!(resumed.result.lot_ids, accepted.lot_ids);
    assert_eq!(resumed.attachment_ids, vec!["92001".to_owned()]);
    assert_no_consignment_rows(pool).await;
}

#[tokio::test]
async fn postgres_phase4_legacy_refund_first_dispatch_direct() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    let (minute, settled_at) = crate::recovery_returns::settlement_minute();
    let sale = crate::recovery_returns::insert_sale(pool, 92_002, settled_at, 1).await;
    let amount = 100;
    let _delivery = crate::settlement_returns::insert_sent_fulfillment(
        pool,
        sale.refund_obligation,
        "settlement_buyer_refund",
        amount,
        minute + Duration::minutes(5),
        "enqueued",
    )
    .await;
    let mid = "9109202";
    let mail_id = crate::settlement_returns::insert_inbound(
        pool,
        mid,
        minute + Duration::minutes(10),
        Some(("MoogleMail", format!("[RTS] Auction buyer refund [{}]", sale.refund_obligation))),
        crate::settlement_returns::evidence(sale.refund_obligation, amount, &[]),
    )
    .await;
    let consumed = store
        .consume_due_settlement_return(Timestamp::now())
        .await
        .expect("旧退款首寄消费")
        .expect("必须由原消费者直接消费");
    assert_eq!(consumed.return_mail_id, mail_id);
    let cases: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.settlement_return_cases WHERE return_mail_id = $1",
    )
    .bind(mail_id)
    .fetch_one(pool)
    .await
    .expect("读取原案");
    assert_eq!(cases, 1, "旧买家退款首寄只建一个原案");
    let takes: i64 = sqlx::query_scalar("SELECT count(*) FROM public.settlement_return_actions")
        .fetch_one(pool)
        .await
        .expect("读取 Take 动作");
    assert_eq!(takes, 0, "交接前不得自动 Take");
    assert_no_consignment_rows(pool).await;
}

fn claim(mailbox_key: &str, token: Uuid, version: i64, cursor: &str) -> MailPollClaim {
    MailPollClaim {
        mailbox_key: mailbox_key.to_owned(),
        worker_id: "worker-i2b2".to_owned(),
        claim_token: token,
        cursor_value: Some(cursor.to_owned()),
        state_version: version,
        paused: false,
        attempt_count: 1,
        lease_until: Timestamp::now() + Duration::hours(1),
    }
}

async fn seed_poll_state(pool: &PgPool, mailbox_key: &str, token: Uuid, cursor: &str) {
    sqlx::query(
        "INSERT INTO public.mail_poll_state
            (mailbox_key, cursor_value, last_poll_at, next_poll_at, consecutive_failures,
             paused, state_version, updated_at, claim_token, claim_owner, claim_expires_at)
         VALUES ($1, $2, clock_timestamp(), clock_timestamp(), 0, false, 1, clock_timestamp(),
                 $3, 'worker-i2b2', clock_timestamp() + interval '1 hour')",
    )
    .bind(mailbox_key)
    .bind(cursor)
    .bind(token)
    .execute(pool)
    .await
    .expect("写入轮询租约");
    sqlx::query(
        "INSERT INTO public.mail_poll_pending_reads
            (mailbox_key, world_id, external_mail_id, sender, subject, received_at)
         VALUES ($1, 'main', '20', 'FakeSellerOne', 'consign', clock_timestamp())",
    )
    .bind(mailbox_key)
    .execute(pool)
    .await
    .expect("写入待读候选");
}

async fn seed_readiness(pool: &PgPool, valid: bool) {
    let (checked_at, valid_until) = if valid {
        ("clock_timestamp()", "clock_timestamp() + interval '1 day'")
    } else {
        ("clock_timestamp() - interval '2 hours'", "clock_timestamp() - interval '1 hour'")
    };
    let sql = format!(
        "INSERT INTO public.consignment_mail_readiness
            (world_id, session_id, service_epoch, policy_digest, allowlist_mode,
             allowed_senders, validator_versions, detail_ready, take_ready, return_ready,
             checked_at, valid_until, version)
         VALUES ('main', '{SESSION}', 1, decode(repeat('35', 32), 'hex'), 'disabled',
                 ARRAY[]::text[], ARRAY[]::text[], true, true, true,
                 {checked_at}, {valid_until}, 1)"
    );
    sqlx::raw_sql(AssertSqlSafe(sql)).execute(pool).await.expect("写入世界就绪");
}

fn observation(observation_id: Uuid, mid: &str) -> EnvelopeObservation {
    let seller = UserId::from_uuid(Uuid::parse_str(USER_1).expect("固定卖家"));
    EnvelopeObservation {
        observation_id,
        session_binding_digest: [0x33; 32],
        manifest_digest: [0x44; 32],
        parser_version: "i2b2-test-parser".to_owned(),
        subject_is_consign: true,
        sender_identity: SenderIdentity::Verified,
        list_detail_consistent: true,
        asset_count: Some(1),
        duplicate_raw_eid: true,
        canonical_alias_ambiguous: false,
        unknown_attachment: false,
        credits: Some(0),
        cod: Some(0),
        returnable: true,
        takeable: Some(true),
        system_rts: false,
        legacy_auction_owner: None,
        items: vec![EnvelopeItemObservation {
            ordinal: 1,
            kind: EnvelopeItemKind::Equipment,
            display_name: Some(format!("Fake Gear {mid}")),
            raw_eid: Some("42".to_owned()),
            canonical_eid: Some("42".to_owned()),
            amount: None,
            world: Some(SettlementWorld::Main),
            seller_user_id: Some(seller),
            tradeable: true,
            history_ok: true,
            protected_ref: Some(Uuid::from_u128(42)),
        }],
    }
}

fn facts(
    key: OriginalMailKey,
    observation: EnvelopeObservation,
    now: Timestamp,
) -> LockedEnvelopeFacts {
    LockedEnvelopeFacts {
        mail_key: Some(key),
        observation,
        reservations: Vec::new(),
        envelope_seller_user_id: Some(UserId::from_uuid(
            Uuid::parse_str(USER_1).expect("固定卖家"),
        )),
        capabilities: EnvelopeCapabilities { safe_original_return: true, take_ready: true },
        read_failure: false,
        already_decided: false,
        now,
    }
}

fn poll_write(
    key: OriginalMailKey,
    observation: EnvelopeObservation,
    claim: MailPollClaim,
    cursor_mid: &str,
) -> PollEnvelopeWrite {
    let now = Timestamp::now();
    let command = DepositCommandService::prepare_decide_envelope(
        facts(key.clone(), observation.clone(), now),
        None,
    )
    .expect("纯构造决定")
    .expect("应首次决定");
    let command_id = command.command_id;
    PollEnvelopeWrite {
        mail_key: key,
        command_id,
        command: Some(command),
        raw: Some(PollRawObservation {
            mail_key: OriginalMailKey::new("12345", SettlementWorld::Main, cursor_mid)
                .expect("原信键"),
            session_id: Uuid::parse_str(SESSION).expect("固定会话"),
            mailbox_key: claim.mailbox_key.clone(),
            remote_observation_id: format!("obs-{cursor_mid}"),
            observed_at: now,
            parser_version: observation.parser_version.clone(),
            evidence_ref: None,
            subject_digest: [0x11; 32],
            body_digest: [0x22; 32],
            sender: Some("FakeSellerOne".to_owned()),
            recipient: Some("FakeBot".to_owned()),
            raw_count: Some(1),
            observation,
            legacy_evidence: LegacyReturnEvidence::default(),
            received_minute_raw: "2026-01-01 03:00".to_owned(),
            received_timezone: "UTC".to_owned(),
            first_seen_at: now,
            season_window_id: None,
        }),
        claim,
        polled_at: now,
        next_poll_at: now + Duration::hours(1),
        cursor_mid: cursor_mid.to_owned(),
    }
}

#[tokio::test]
async fn postgres_quarantine_prefix_advances_once() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    succeeds(pool, &(users_and_key() + &listing_awaiting_deposit() + &mail_session())).await;
    sqlx::query(
        "INSERT INTO public.consignment_mail_readiness
            (world_id, session_id, service_epoch, policy_digest, allowlist_mode,
             allowed_senders, validator_versions, detail_ready, take_ready, return_ready,
             checked_at, valid_until, version)
         VALUES ('main', $1, 1, decode(repeat('35', 32), 'hex'), 'disabled',
                 ARRAY[]::text[], ARRAY[]::text[], true, true, true,
                 clock_timestamp(), clock_timestamp() + interval '1 day', 1)",
    )
    .bind(Uuid::parse_str(SESSION).expect("固定会话"))
    .execute(pool)
    .await
    .expect("写入世界就绪");
    let token = Uuid::new_v4();
    seed_poll_state(pool, "i2b2-mailbox", token, "10").await;
    let key = OriginalMailKey::new("12345", SettlementWorld::Main, "20").expect("原信键");
    let write = poll_write(
        key,
        observation(Uuid::new_v4(), "20"),
        claim("i2b2-mailbox", token, 1, "10"),
        "20",
    );
    let receipt: EnvelopeReceipt =
        store.decide_envelope_in_poll(write.clone()).await.expect("首次隔离并推进前缀");
    assert_eq!(
        receipt.disposition,
        auction_application::consignment::ReceiptDisposition::Quarantined
    );
    assert!(receipt.quarantine.is_some());
    let state: (Option<String>, i64, Option<Uuid>) = sqlx::query_as(
        "SELECT cursor_value, state_version, claim_token
         FROM public.mail_poll_state WHERE mailbox_key = 'i2b2-mailbox'",
    )
    .fetch_one(pool)
    .await
    .expect("读取前缀");
    assert_eq!(state.0.as_deref(), Some("20"));
    assert_eq!(state.1, 2);
    assert!(state.2.is_none(), "提交后必须释放租约");
    let counts: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM public.mail_initial_decisions
              WHERE bot_uid='12345' AND world_id='main' AND external_mid='20'),
            (SELECT count(*) FROM public.mail_quarantines
              WHERE bot_uid='12345' AND world_id='main' AND external_mid='20'
                AND state='active'),
            (SELECT count(*) FROM public.mail_poll_pending_reads
              WHERE mailbox_key='i2b2-mailbox'),
            (SELECT count(*) FROM public.inbound_mail)",
    )
    .fetch_one(pool)
    .await
    .expect("读取决定、隔离与前缀");
    assert_eq!(counts, (1, 1, 0, 0));
    let replay = store.decide_envelope_in_poll(write).await;
    assert!(replay.is_err(), "旧 claim 不能第二次成功推进");
    let after: (i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM public.mail_initial_decisions
              WHERE bot_uid='12345' AND world_id='main' AND external_mid='20'),
            (SELECT count(*) FROM public.mail_quarantines
              WHERE bot_uid='12345' AND world_id='main' AND external_mid='20')",
    )
    .fetch_one(pool)
    .await
    .expect("读取重放后的决定数");
    assert_eq!(after, (1, 1), "重放不得产生第二份隔离");

    // 故障注入：游标 CAS 必须与决定同事务；使用陈旧 state_version 时决定整体回滚。
    let rollback_token = Uuid::new_v4();
    seed_poll_state(pool, "i2b2-rollback-mailbox", rollback_token, "10").await;
    let rollback_key =
        OriginalMailKey::new("12345", SettlementWorld::Main, "21").expect("回滚原信键");
    let mut rollback_claim = claim("i2b2-rollback-mailbox", rollback_token, 1, "10");
    rollback_claim.state_version = 99;
    let rollback_write =
        poll_write(rollback_key, observation(Uuid::new_v4(), "21"), rollback_claim, "21");
    let error = store.decide_envelope_in_poll(rollback_write).await.expect_err("陈旧租约必须失败");
    assert!(matches!(error, ConsignmentError::LeaseLost | ConsignmentError::Unavailable));
    let rolled_back: (i64, i64, Option<String>) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM public.mail_initial_decisions
              WHERE bot_uid='12345' AND world_id='main' AND external_mid='21'),
            (SELECT count(*) FROM public.mail_originals
              WHERE bot_uid='12345' AND world_id='main' AND external_mid='21'),
            (SELECT cursor_value FROM public.mail_poll_state
              WHERE mailbox_key='i2b2-rollback-mailbox')",
    )
    .fetch_one(pool)
    .await
    .expect("读取回滚事实");
    assert_eq!(rolled_back, (0, 0, Some("10".to_owned())));
}

#[tokio::test]
async fn postgres_poll_gate_and_preflight_require_live_readiness() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    succeeds(pool, &(users_and_key() + &listing_awaiting_deposit() + &mail_session())).await;
    let key = OriginalMailKey::new("12345", SettlementWorld::Main, "301").expect("原信键");
    let observation = observation(Uuid::new_v4(), "301");
    let sender = Some("FakeSellerOne".to_owned());

    assert!(
        !store.consignment_candidates_enabled(SettlementWorld::Main).await.expect("读取门控"),
        "没有就绪投影时必须关闭寄售候选"
    );
    assert!(matches!(
        store.load_envelope_preflight(key.clone(), observation.clone(), sender.clone(), None).await,
        Err(ConsignmentError::Unavailable)
    ));

    // 已过期就绪必须失败关闭；插入过去时刻后由新版本推进到有效，避免 checked_at 回退。
    seed_readiness(pool, false).await;
    assert!(
        !store.consignment_candidates_enabled(SettlementWorld::Main).await.expect("读取门控"),
        "valid_until 已过时必须关闭寄售候选"
    );
    assert!(matches!(
        store.load_envelope_preflight(key.clone(), observation.clone(), sender.clone(), None).await,
        Err(ConsignmentError::Unavailable)
    ));

    sqlx::query(
        "UPDATE public.consignment_mail_readiness
         SET checked_at = clock_timestamp(),
             valid_until = clock_timestamp() + interval '1 day',
             version = version + 1
         WHERE world_id = 'main'",
    )
    .execute(pool)
    .await
    .expect("续期就绪");
    assert!(
        store.consignment_candidates_enabled(SettlementWorld::Main).await.expect("读取门控"),
        "就绪有效时必须允许寄售候选"
    );
    let preflight = store
        .load_envelope_preflight(key, observation, sender, None)
        .await
        .expect("就绪有效时预读");
    assert_eq!(
        preflight.session_id,
        Uuid::parse_str(SESSION).expect("固定会话"),
        "预读必须引用签收会话"
    );
}

#[tokio::test]
async fn postgres_poll_decision_replans_when_reservation_appears_after_preflight() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    succeeds(pool, &(users_and_key() + &listing_awaiting_deposit() + &mail_session())).await;
    seed_readiness(pool, true).await;

    let key = OriginalMailKey::new("12345", SettlementWorld::Main, "20").expect("原信键");
    let observed = observation(Uuid::new_v4(), "20");
    let sender = Some("FakeSellerOne".to_owned());
    let preflight = store
        .load_envelope_preflight(key.clone(), observed.clone(), sender.clone(), None)
        .await
        .expect("锁外预读");
    assert!(preflight.reservations.is_empty(), "预读时尚无预约");

    // 预读之后出现同卖家、同 EID 的新 active 预约；锁内重读必须发现集合已变化。
    succeeds(pool, &reservation_issued()).await;

    let token = Uuid::new_v4();
    seed_poll_state(pool, "i2b2-replan-mailbox", token, "10").await;
    let write = poll_write(key, observed, claim("i2b2-replan-mailbox", token, 1, "10"), "20");
    let error =
        store.decide_envelope_in_poll(write).await.expect_err("预读后新增预约必须退出重规划");
    assert!(matches!(error, ConsignmentError::Unavailable));
    let rolled_back: (i64, i64, Option<String>) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM public.mail_initial_decisions
              WHERE bot_uid='12345' AND world_id='main' AND external_mid='20'),
            (SELECT count(*) FROM public.mail_originals
              WHERE bot_uid='12345' AND world_id='main' AND external_mid='20'),
            (SELECT cursor_value FROM public.mail_poll_state
              WHERE mailbox_key='i2b2-replan-mailbox')",
    )
    .fetch_one(pool)
    .await
    .expect("读取重规划回滚事实");
    assert_eq!(rolled_back, (0, 0, Some("10".to_owned())));

    // 对照：同一预读窗口内没有新预约时，决定与游标正常提交。
    let control = TestDatabase::create().await.expect("克隆对照模板库");
    let control_store = control.store().await.expect("对照运行账号");
    let control_pool = control_store.postgres_pool();
    succeeds(control_pool, &(users_and_key() + &listing_awaiting_deposit() + &mail_session()))
        .await;
    seed_readiness(control_pool, true).await;
    let control_key =
        OriginalMailKey::new("12345", SettlementWorld::Main, "21").expect("对照原信键");
    let control_observed = observation(Uuid::new_v4(), "21");
    let control_preflight = control_store
        .load_envelope_preflight(control_key.clone(), control_observed.clone(), sender, None)
        .await
        .expect("对照预读");
    assert!(control_preflight.reservations.is_empty());
    let control_token = Uuid::new_v4();
    seed_poll_state(control_pool, "i2b2-control-mailbox", control_token, "10").await;
    let control_write = poll_write(
        control_key,
        control_observed,
        claim("i2b2-control-mailbox", control_token, 1, "10"),
        "21",
    );
    let receipt = control_store
        .decide_envelope_in_poll(control_write)
        .await
        .expect("没有新预约时必须正常提交");
    assert_eq!(
        receipt.disposition,
        auction_application::consignment::ReceiptDisposition::Quarantined
    );
}
