//! 隔离重检命中旧拍卖 owner 后的自动交接，以及关闭后续办与无 poll 租约的 PG 行为。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use auction_application::consignment::{
    AppendQuarantineObservationCommand, AutoReleaseBinding, DecideEnvelopeCommand,
    DepositCommandService, DepositOperation, EnvelopeStore, LegacyAuctionOwnerResolver,
    system_command_id,
};
use auction_application::core::RequestHash;
use auction_application::mail::{
    DispositionRoundRequest, QuarantineRecheckReadError, QuarantineRecheckReader,
    QuarantineRecheckRound, QuarantineRecheckService, QuarantineRecheckStore,
};
use auction_domain::SettlementWorld;
use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::deposit::{
    EnvelopeCapabilities, EnvelopeDecision, EnvelopeItemKind, EnvelopeItemObservation,
    EnvelopeObservation, LegacyAuctionOwner, LockedEnvelopeFacts, QuarantineReason, ReleaseKind,
    SenderIdentity,
};
use auction_storage::DatabaseStore;
use auction_time::Timestamp;
use serde_json::{Value, json};
use sqlx::{AssertSqlSafe, PgPool};
use uuid::Uuid;

use super::envelopes::{repository, seed_base, worker_command};
use crate::recovery_returns::{insert_awaiting_recovery, insert_sale, settlement_minute};
use crate::settlement_returns::insert_sent_fulfillment;
use crate::support::TestDatabase;

const OBSERVATION: &str = "8f000000-0000-0000-0000-000000000011";
const MAILBOX: &str = "fake-mailbox";
const MID: &str = "555";
const BOT_UID: &str = "12345";

struct OwnerReader {
    observation: EnvelopeObservation,
    remote_observation_id: String,
}

impl QuarantineRecheckReader for OwnerReader {
    fn reread(
        &self,
        _claim: &auction_application::mail::QuarantineRecheckClaim,
        _deadline: Instant,
    ) -> auction_application::consignment::ConsignmentFuture<
        '_,
        Result<auction_application::mail::QuarantineReobservation, QuarantineRecheckReadError>,
    > {
        let observation = self.observation.clone();
        let remote_observation_id = self.remote_observation_id.clone();
        Box::pin(async move {
            Ok(Ok(auction_application::mail::QuarantineReobservation {
                remote_observation_id,
                observed_at: Timestamp::parse_from_rfc3339("2026-01-01T05:00:00Z")
                    .expect("固定观察时刻"),
                observation,
                evidence_ref: None,
            }))
        })
    }
}

fn key() -> OriginalMailKey {
    OriginalMailKey::new(BOT_UID, SettlementWorld::Main, MID).expect("原信键")
}

fn equipment_item(external_eid: &str) -> EnvelopeItemObservation {
    EnvelopeItemObservation {
        ordinal: 1,
        kind: EnvelopeItemKind::Equipment,
        display_name: Some("Fake Returned Gear".to_owned()),
        raw_eid: Some(external_eid.to_owned()),
        canonical_eid: Some(external_eid.to_owned()),
        amount: None,
        world: Some(SettlementWorld::Main),
        seller_user_id: None,
        tradeable: true,
        history_ok: true,
        protected_ref: None,
    }
}

fn observation(
    _obligation_id: Uuid,
    _subject: String,
    items: Vec<EnvelopeItemObservation>,
) -> EnvelopeObservation {
    EnvelopeObservation {
        observation_id: Uuid::parse_str(OBSERVATION).expect("观察"),
        session_binding_digest: [0x33; 32],
        manifest_digest: [0x6d; 32],
        parser_version: "parser-legacy".to_owned(),
        subject_is_consign: false,
        sender_identity: SenderIdentity::Verified,
        list_detail_consistent: true,
        asset_count: Some(u32::try_from(items.len()).expect("件数")),
        duplicate_raw_eid: false,
        canonical_alias_ambiguous: false,
        unknown_attachment: false,
        credits: Some(0),
        cod: Some(0),
        returnable: false,
        takeable: Some(false),
        system_rts: true,
        legacy_auction_owner: None,
        items,
    }
}

fn manifest(items: &[EnvelopeItemObservation]) -> Value {
    Value::Array(
        items
            .iter()
            .map(|item| {
                json!({
                    "ordinal": item.ordinal,
                    "kind": "equipment",
                    "name": item.display_name,
                    "raw_eid": item.raw_eid,
                    "canonical_eid": item.canonical_eid,
                    "amount": Value::Null,
                })
            })
            .collect(),
    )
}

fn derived_facts_with_identity(
    observation: &EnvelopeObservation,
    obligation_id: Uuid,
    subject: &str,
    sender_identity: &str,
) -> Value {
    json!({
        "subject_is_consign": false,
        "sender_identity": sender_identity,
        "list_detail_consistent": true,
        "duplicate_raw_eid": false,
        "canonical_alias_ambiguous": false,
        "unknown_attachment": false,
        "system_rts": true,
        "legacy_auction_owner": Value::Null,
        "legacy_obligation_id": obligation_id.to_string(),
        "legacy_subject": subject,
        "returned_from": Value::Null,
        "body_digest": Value::Null,
        "dispatch_reference": Value::Null,
        "received_minute_raw": "2026-01-01T03:00",
        "asset_count": observation.asset_count,
        "items": observation.items.iter().map(|item| json!({
            "ordinal": item.ordinal,
            "world": "main",
            "tradeable": item.tradeable,
            "history_ok": item.history_ok,
        })).collect::<Vec<_>>(),
    })
}

async fn insert_observation(
    pool: &PgPool,
    observation: &EnvelopeObservation,
    obligation_id: Uuid,
    subject: &str,
) {
    insert_observation_with_identity(pool, observation, obligation_id, subject, "verified").await;
}

async fn insert_observation_with_identity(
    pool: &PgPool,
    observation: &EnvelopeObservation,
    obligation_id: Uuid,
    subject: &str,
    sender_identity: &str,
) {
    sqlx::query(
        "INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
             evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
             returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
             season_window_id)
         VALUES ($1, $2, 'main', $3, 'legacy-rts-source', $4, $5, NULL, NULL,
                 '2026-01-01T04:00:00Z', $6, NULL,
                 decode(repeat('72', 32), 'hex'), decode(repeat('73', 32), 'hex'), $7,
                 'MoogleMail', 'FakeBot', false, false, $8, $9, NULL, $10, $11, NULL)",
    )
    .bind(observation.observation_id)
    .bind(BOT_UID)
    .bind(MID)
    .bind(Uuid::parse_str(super::support::SESSION).expect("会话"))
    .bind(MAILBOX)
    .bind(&observation.parser_version)
    .bind(observation.manifest_digest.as_slice())
    .bind(observation.credits)
    .bind(observation.cod)
    .bind(manifest(&observation.items))
    .bind(derived_facts_with_identity(observation, obligation_id, subject, sender_identity))
    .execute(pool)
    .await
    .expect("写入带旧证据的隔离源观察");
}

fn decide_quarantine(observation: EnvelopeObservation, name: &str) -> DecideEnvelopeCommand {
    DecideEnvelopeCommand {
        command_id: worker_command(name),
        request_hash: RequestHash::new([0x51; 32]),
        mail_key: key(),
        facts: LockedEnvelopeFacts {
            mail_key: Some(key()),
            observation,
            reservations: Vec::new(),
            envelope_seller_user_id: None,
            capabilities: EnvelopeCapabilities { safe_original_return: false, take_ready: false },
            read_failure: false,
            already_decided: false,
            now: Timestamp::UNIX_EPOCH,
        },
        decision: EnvelopeDecision::Quarantine(QuarantineReason::SuspectedSystemRts),
    }
}

fn service(
    repository: &auction_storage::DepositRepository,
    owner: LegacyAuctionOwner,
) -> QuarantineRecheckService {
    let mut reobserved = observation(Uuid::nil(), String::new(), Vec::new());
    reobserved.legacy_auction_owner = Some(owner);
    let reader = Arc::new(OwnerReader {
        observation: reobserved,
        remote_observation_id: format!("legacy-owner-{}", owner.as_str()),
    });
    let commands = command_service(repository);
    QuarantineRecheckService::new(commands, Arc::new(repository.clone()), reader)
}

fn command_service(repository: &auction_storage::DepositRepository) -> Arc<DepositCommandService> {
    Arc::new(DepositCommandService::new(
        Arc::new(repository.clone()),
        Arc::new(repository.clone()),
        Arc::new(repository.clone()),
        Arc::new(repository.clone()),
        Arc::new(repository.clone()),
    ))
}

/// 经真实 `DatabaseStore` 判定器重读；不把 owner 直接写进假观察。
struct ResolverReader {
    store: DatabaseStore,
    observation: EnvelopeObservation,
    remote_observation_id: String,
}

impl QuarantineRecheckReader for ResolverReader {
    fn reread(
        &self,
        claim: &auction_application::mail::QuarantineRecheckClaim,
        _deadline: Instant,
    ) -> auction_application::consignment::ConsignmentFuture<
        '_,
        Result<auction_application::mail::QuarantineReobservation, QuarantineRecheckReadError>,
    > {
        let previous_observation_id = claim.previous_observation_id;
        let mail_key = claim.mail_key.clone();
        Box::pin(async move {
            let mut observation = self.observation.clone();
            // 与真实读取器一致：观察身份绑定隔离内上一行，旧 owner 由存储层判定。
            observation.observation_id = previous_observation_id;
            let owner = match self.store.resolve(&mail_key, &observation).await {
                Ok(owner) => owner,
                Err(_) => return Ok(Err(QuarantineRecheckReadError::TemporaryUnavailable)),
            };
            observation.legacy_auction_owner = owner;
            Ok(Ok(auction_application::mail::QuarantineReobservation {
                remote_observation_id: self.remote_observation_id.clone(),
                observed_at: Timestamp::parse_from_rfc3339("2026-01-01T05:00:00Z")
                    .expect("固定观察时刻"),
                observation,
                evidence_ref: None,
            }))
        })
    }
}

fn resolver_service(
    repository: &auction_storage::DepositRepository,
    store: DatabaseStore,
    observation: EnvelopeObservation,
) -> QuarantineRecheckService {
    let reader = Arc::new(ResolverReader {
        store,
        observation,
        remote_observation_id: "legacy-resolved-observation".to_owned(),
    });
    let commands = command_service(repository);
    QuarantineRecheckService::new(commands, Arc::new(repository.clone()), reader)
}

async fn round_request(pool: &PgPool) -> DispositionRoundRequest {
    let now: Timestamp =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(pool).await.expect("数据库时刻");
    DispositionRoundRequest {
        worker_id: "recheck-worker".to_owned(),
        now,
        lease_until: now + auction_time::Duration::hours(1),
        deadline: Instant::now() + StdDuration::from_secs(30),
    }
}

async fn count(pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(AssertSqlSafe(sql.to_owned())).fetch_one(pool).await.expect("读取计数")
}

async fn quarantine_state(pool: &PgPool) -> (String, i64) {
    sqlx::query_as("SELECT state, version FROM public.mail_quarantines")
        .fetch_one(pool)
        .await
        .expect("读取隔离状态")
}

#[tokio::test]
async fn postgres_owner_hint_hands_off_once() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let (minute, settled_at) = settlement_minute();
    let sale = insert_sale(pool, 7_200_001, settled_at, 1).await;
    let delivery_id = insert_sent_fulfillment(
        pool,
        sale.seller_obligation,
        "settlement_seller_payout",
        90,
        minute + auction_time::Duration::minutes(10),
        "enqueued",
    )
    .await;
    let subject = format!("[RTS] Auction seller payout [{}]", sale.seller_obligation);
    let mut sender = observation(sale.seller_obligation, subject.clone(), Vec::new());
    // 兼容行的旧证据金额必须来自观察事实，和卖款派发金额一致。
    sender.credits = Some(90);
    insert_observation(pool, &sender, sale.seller_obligation, &subject).await;
    let first = repository
        .decide_envelope(decide_quarantine(sender, "legacy-owner-return-case"))
        .await
        .expect("首次隔离");
    assert!(first.quarantine.is_some());

    let service = service(&repository, LegacyAuctionOwner::SellerPayout);
    let round = service.run_once(round_request(pool).await).await.expect("自动交接");
    assert!(matches!(round, QuarantineRecheckRound::Appended { quarantine_version: 2, .. }));
    assert_eq!(quarantine_state(pool).await.0, "released");

    let state = (
        count(pool, "SELECT count(*) FROM public.inbound_mail").await,
        count(pool, "SELECT count(*) FROM public.mail_original_links").await,
        count(pool, "SELECT count(*) FROM public.settlement_return_evidence").await,
        count(pool, "SELECT count(*) FROM public.settlement_return_cases").await,
        count(pool, "SELECT count(*) FROM public.mail_quarantine_releases").await,
        count(pool, "SELECT count(*) FROM public.mail_final_dispositions").await,
        count(pool, "SELECT count(*) FROM public.mail_disposition_work").await,
        count(pool, "SELECT count(*) FROM public.settlement_funding_entries").await,
    );
    assert_eq!(state, (1, 1, 1, 1, 1, 1, 0, 0));
    let dispatch: Option<Uuid> =
        sqlx::query_scalar("SELECT delivery_id FROM public.settlement_return_evidence")
            .fetch_one(pool)
            .await
            .expect("原案证据");
    assert_eq!(dispatch, Some(delivery_id));
    let metadata: Value = sqlx::query_scalar("SELECT decision_metadata FROM public.inbound_mail")
        .fetch_one(pool)
        .await
        .expect("兼容入站证据");
    assert_eq!(metadata["obligation_id"], json!(sale.seller_obligation.to_string()));
    assert_eq!(metadata["credits"], json!(90));
    let linked_inbound: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.inbound_mail
         WHERE id = (SELECT inbound_id FROM public.mail_original_links)",
    )
    .fetch_one(pool)
    .await
    .expect("链接指向入站行");
    assert_eq!(linked_inbound, 1);

    let again = service.run_once(round_request(pool).await).await.expect("再次运行");
    assert_eq!(again, QuarantineRecheckRound::Idle);
    assert_eq!(quarantine_state(pool).await, ("released".to_owned(), 3));
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_cases").await, 1);
}

#[tokio::test]
async fn postgres_owner_hint_resolver_hands_off_once() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let (minute, settled_at) = settlement_minute();
    let sale = insert_sale(pool, 7_200_011, settled_at, 1).await;
    insert_sent_fulfillment(
        pool,
        sale.seller_obligation,
        "settlement_seller_payout",
        90,
        minute + auction_time::Duration::minutes(10),
        "enqueued",
    )
    .await;
    let subject = format!("[RTS] Auction seller payout [{}]", sale.seller_obligation);
    let mut sender = observation(sale.seller_obligation, subject.clone(), Vec::new());
    sender.credits = Some(90);
    insert_observation(pool, &sender, sale.seller_obligation, &subject).await;
    repository
        .decide_envelope(decide_quarantine(sender.clone(), "resolver-owner-return-case"))
        .await
        .expect("首次隔离");

    // 正例：规范系统退件主题 + 本地卖款义务经真实解析器得到 SellerPayout。
    let resolved = store.resolve(&key(), &sender).await.expect("旧 owner 判定");
    assert_eq!(resolved, Some(LegacyAuctionOwner::SellerPayout));

    let service = resolver_service(&repository, store.clone(), sender);
    let round = service.run_once(round_request(pool).await).await.expect("自动交接");
    assert!(matches!(round, QuarantineRecheckRound::Appended { quarantine_version: 2, .. }));
    assert_eq!(quarantine_state(pool).await.0, "released");
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_cases").await, 1);
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_disposition_work").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_funding_entries").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.inbound_mail").await, 1);
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_original_links").await, 1);

    let again = service.run_once(round_request(pool).await).await.expect("再次运行");
    assert_eq!(again, QuarantineRecheckRound::Idle);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_cases").await, 1);
}

#[tokio::test]
async fn postgres_recheck_reuses_same_system_release_after_lost_completion() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let (minute, settled_at) = settlement_minute();
    let sale = insert_sale(pool, 7_200_013, settled_at, 1).await;
    insert_sent_fulfillment(
        pool,
        sale.seller_obligation,
        "settlement_seller_payout",
        90,
        minute + auction_time::Duration::minutes(10),
        "enqueued",
    )
    .await;
    let subject = format!("[RTS] Auction seller payout [{}]", sale.seller_obligation);
    let mut sender = observation(sale.seller_obligation, subject.clone(), Vec::new());
    sender.credits = Some(90);
    insert_observation(pool, &sender, sale.seller_obligation, &subject).await;
    let first = repository
        .decide_envelope(decide_quarantine(sender.clone(), "recheck-lost-completion"))
        .await
        .expect("首次隔离");
    let quarantine = first.quarantine.expect("隔离引用");

    // 模拟上一轮已追加观察并完成系统交接，但在收口工作前退出。
    let first_request = round_request(pool).await;
    let first_claim = repository
        .claim_due("recheck-before-crash".to_owned(), first_request.now, first_request.lease_until)
        .await
        .expect("领取首次重检")
        .expect("自动排队的重检");
    let mut reobserved = sender.clone();
    reobserved.observation_id = first_claim.previous_observation_id;
    reobserved.legacy_auction_owner = Some(LegacyAuctionOwner::SellerPayout);
    let appended = repository
        .append_quarantine_observation(AppendQuarantineObservationCommand {
            command_id: worker_command("recheck-lost-completion-append"),
            request_hash: RequestHash::new([0x61; 32]),
            quarantine_id: quarantine.quarantine_id,
            remote_observation_id: "owner-observation-before-crash".to_owned(),
            observed_at: Timestamp::parse_from_rfc3339("2026-01-01T05:00:00Z")
                .expect("固定观察时刻"),
            observation: reobserved,
            evidence_ref: None,
        })
        .await
        .expect("追加首次重检观察");
    let commands = command_service(&repository);
    let released = commands
        .release_quarantine_auto(
            quarantine.quarantine_id,
            appended.quarantine_version,
            appended.observation_id,
            ReleaseKind::AuctionReturnCase,
            AutoReleaseBinding {
                observation_digest: appended.digest,
                source_ref: "legacy-auction-owner:seller_payout".to_owned(),
            },
        )
        .await
        .expect("首次系统交接");
    let expected_command = system_command_id(DepositOperation::ReleaseQuarantine, &{
        let mut key = quarantine.quarantine_id.as_bytes().to_vec();
        key.extend_from_slice(appended.observation_id.as_bytes());
        key
    });
    assert_eq!(released.command_id, expected_command);
    let release_observation: Uuid = sqlx::query_scalar(
        "SELECT observation_id FROM public.mail_quarantine_releases
         WHERE quarantine_id = $1",
    )
    .bind(quarantine.quarantine_id)
    .fetch_one(pool)
    .await
    .expect("读取解除观察");
    assert_eq!(release_observation, appended.observation_id);
    let latest_observation: Uuid = sqlx::query_scalar(
        "SELECT id FROM public.mail_envelope_observations
         WHERE quarantine_id = $1
         ORDER BY quarantine_version DESC, observed_at DESC, id DESC
         LIMIT 1",
    )
    .bind(quarantine.quarantine_id)
    .fetch_one(pool)
    .await
    .expect("读取最新观察");
    assert_eq!(latest_observation, appended.observation_id);
    sqlx::query(
        "UPDATE public.mail_quarantine_rechecks
         SET lease_until = clock_timestamp() - interval '1 second'
         WHERE id = $1",
    )
    .bind(first_claim.work_id)
    .execute(pool)
    .await
    .expect("模拟工作收口前退出并过租");

    let service = resolver_service(&repository, store.clone(), sender);
    let round = service.run_once(round_request(pool).await).await.expect("重新领取重检");
    assert_eq!(
        round,
        QuarantineRecheckRound::Appended {
            quarantine_version: appended.quarantine_version,
            observation_id: appended.observation_id,
        }
    );
    let recheck_state: String =
        sqlx::query_scalar("SELECT state FROM public.mail_quarantine_rechecks WHERE id = $1")
            .bind(first_claim.work_id)
            .fetch_one(pool)
            .await
            .expect("读取重检状态");
    assert_eq!(recheck_state, "done", "同一系统解除已完成时必须直接收口，不转 reconcile");
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_quarantine_releases").await, 1);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_cases").await, 1);
    let quarantine_observations: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.mail_envelope_observations WHERE quarantine_id = $1",
    )
    .bind(quarantine.quarantine_id)
    .fetch_one(pool)
    .await
    .expect("读取隔离观察数");
    assert_eq!(quarantine_observations, 2);
}

#[tokio::test]
async fn postgres_owner_hint_resolver_requires_verified_system_return() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let (_, settled_at) = settlement_minute();
    let sale = insert_sale(pool, 7_200_012, settled_at, 1).await;
    let subject = format!("[RTS] Auction seller payout [{}]", sale.seller_obligation);
    let mut sender = observation(sale.seller_obligation, subject.clone(), Vec::new());
    sender.sender_identity = SenderIdentity::Unverified;
    insert_observation_with_identity(pool, &sender, sale.seller_obligation, &subject, "unverified")
        .await;
    repository
        .decide_envelope(decide_quarantine(sender.clone(), "resolver-unverified-owner"))
        .await
        .expect("首次隔离");

    // 反例：身份未认证时存储层判定必须失败关闭。
    let resolved = store.resolve(&key(), &sender).await.expect("旧 owner 判定");
    assert_eq!(resolved, None);

    let service = resolver_service(&repository, store.clone(), sender);
    let round = service.run_once(round_request(pool).await).await.expect("重检保持隔离");
    assert!(matches!(round, QuarantineRecheckRound::Appended { .. }));
    assert_eq!(quarantine_state(pool).await.0, "active");
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_quarantine_releases").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_cases").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_disposition_work").await, 0);
}

#[tokio::test]
async fn postgres_owner_hint_recovery_hands_off_once() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let (_, settled_at) = settlement_minute();
    let sale = insert_sale(pool, 7_200_002, settled_at, 1).await;
    let recovery_id = insert_awaiting_recovery(pool, &sale, settled_at).await;
    let subject = format!("[RTS] Auction equipment delivery [{}]", sale.payment_obligation);
    let sender = observation(
        sale.payment_obligation,
        subject.clone(),
        vec![equipment_item(&sale.external_equipment_id)],
    );
    insert_observation(pool, &sender, sale.payment_obligation, &subject).await;
    repository
        .decide_envelope(decide_quarantine(sender, "legacy-owner-recovery"))
        .await
        .expect("首次隔离");

    let service = service(&repository, LegacyAuctionOwner::WinnerEquipmentCod);
    let round = service.run_once(round_request(pool).await).await.expect("自动交接");
    assert!(matches!(round, QuarantineRecheckRound::Appended { quarantine_version: 2, .. }));
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_handoffs").await,
        1
    );
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_evidence").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_disposition_work").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_funding_entries").await, 0);
    assert_eq!(quarantine_state(pool).await.0, "released");
    let (recovery, obligation): (Uuid, Uuid) = sqlx::query_as(
        "SELECT recovery_id, obligation_id FROM public.settlement_recovery_return_handoffs",
    )
    .fetch_one(pool)
    .await
    .expect("恢复接管");
    assert_eq!((recovery, obligation), (recovery_id, sale.payment_obligation));
    let again = service.run_once(round_request(pool).await).await.expect("再次运行");
    assert_eq!(again, QuarantineRecheckRound::Idle);
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_handoffs").await,
        1
    );
}

#[tokio::test]
async fn postgres_legacy_inbound_conflict_keeps_quarantine_active() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let (minute, settled_at) = settlement_minute();
    let sale = insert_sale(pool, 7_200_003, settled_at, 1).await;
    insert_sent_fulfillment(
        pool,
        sale.seller_obligation,
        "settlement_seller_payout",
        90,
        minute + auction_time::Duration::minutes(10),
        "enqueued",
    )
    .await;
    let subject = format!("[RTS] Auction seller payout [{}]", sale.seller_obligation);
    let sender = observation(sale.seller_obligation, subject.clone(), Vec::new());
    insert_observation(pool, &sender, sale.seller_obligation, &subject).await;
    repository
        .decide_envelope(decide_quarantine(sender, "legacy-owner-conflict"))
        .await
        .expect("首次隔离");
    let inbound_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("auction/inbound-mail-v1/{MAILBOX}/main/{MID}").as_bytes(),
    );
    sqlx::query(
        "INSERT INTO public.inbound_mail
            (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
             received_at, status, decision_code, decision_metadata, processed_at,
             sender, subject)
         VALUES ($1, $2, $3, 'main', 'delivery', decode(repeat('99', 32), 'hex'),
                 clock_timestamp() - interval '1 hour', 'accepted',
                 'settlement_recovery_returned', '{}'::jsonb,
                 clock_timestamp(), 'MoogleMail', $4)",
    )
    .bind(inbound_id)
    .bind(MAILBOX)
    .bind(MID)
    .bind(&subject)
    .execute(pool)
    .await
    .expect("写入不一致的旧入站行");

    let service = service(&repository, LegacyAuctionOwner::SellerPayout);
    let round = service.run_once(round_request(pool).await).await.expect("交接失败关闭");
    assert_eq!(round, QuarantineRecheckRound::Retried);
    assert_eq!(quarantine_state(pool).await.0, "active");
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_quarantine_releases").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_evidence").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_original_links").await, 0);
}

#[tokio::test]
async fn postgres_recheck_claim_needs_no_poll_lease() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let subject = format!(
        "[RTS] Auction equipment delivery [{}]",
        Uuid::parse_str("10000000-0000-0000-0000-000000000099").expect("义务")
    );
    let observation = observation(
        Uuid::parse_str("10000000-0000-0000-0000-000000000099").expect("义务"),
        subject.clone(),
        Vec::new(),
    );
    insert_observation(
        pool,
        &observation,
        Uuid::parse_str("10000000-0000-0000-0000-000000000099").expect("义务"),
        &subject,
    )
    .await;
    repository
        .decide_envelope(decide_quarantine(observation, "recheck-without-poll-lease"))
        .await
        .expect("首次隔离");
    let poll_leases: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.mail_poll_state WHERE mailbox_key = $1")
            .bind(MAILBOX)
            .fetch_one(pool)
            .await
            .expect("读取 poll 租约");
    assert_eq!(poll_leases, 0);
    let request = round_request(pool).await;
    let claim = repository
        .claim_due(request.worker_id, request.now, request.lease_until)
        .await
        .expect("领取重检")
        .expect("自动排队的重检");
    assert!(!claim.previous_observation_id.is_nil());
    assert!(claim.previous_binding.is_some());
}

#[tokio::test]
async fn postgres_closed_consignment_work_continues() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    assert!(!store.has_consignment_work().await.expect("空库存量"));
    seed_base(pool).await;
    let repository = repository(&database).await;
    let decision_command = worker_command("closed-work-return");
    repository
        .decide_envelope(super::envelopes::return_command(decision_command))
        .await
        .expect("决定原样退回");
    let (work_id, delivery_id, decision_id, observation_id): (Uuid, Uuid, Uuid, Uuid) =
        sqlx::query_as(
            "SELECT id, delivery_id, decision_id, expected_observation_id
             FROM public.mail_disposition_work",
        )
        .fetch_one(pool)
        .await
        .expect("处置工作");

    // 删除活动挂单与预约，只留下一条已起写未收口的处置工作。
    let maintenance = database.maintenance_store().await.expect("维护账号");
    sqlx::query(
        "DELETE FROM public.consignment_equipment_claims
         WHERE reservation_id = $1",
    )
    .bind(Uuid::parse_str(super::support::RESERVATION).expect("预约"))
    .execute(maintenance.postgres_pool())
    .await
    .expect("删除测试占用");
    sqlx::query("DELETE FROM public.consignment_deposit_reservations WHERE id = $1")
        .bind(Uuid::parse_str(super::support::RESERVATION).expect("预约"))
        .execute(maintenance.postgres_pool())
        .await
        .expect("删除测试预约");
    sqlx::query(
        "WITH wall_clock AS MATERIALIZED (SELECT clock_timestamp() AS at),
              command_row AS (
                  INSERT INTO public.consignment_commands
                      (command_id, actor_kind, actor_ref, operation, request_target,
                       request_digest, replay_key_id, listing_id, result_status, result_code,
                       result_body, created_at)
                  SELECT $2, 'admin', 'fake-admin', 'admin_delist', $1,
                         decode(repeat('c1', 32), 'hex'), NULL, $1, 200, 'ok',
                         jsonb_build_object(
                             'command_id', $2,
                             'operation', 'admin_delist',
                             'listing_id', $1,
                             'listing_version', 4,
                             'state', 'closed',
                             'disposition', 'closed',
                             'price', 101),
                         at
                  FROM wall_clock
                  RETURNING command_id
              ),
              event_row AS (
                  INSERT INTO public.consignment_events
                      (listing_id, state_version, command_id, actor_kind, event_kind,
                       from_state, to_state, reason_code, price, occurred_at)
                  SELECT $1, 4, command_id, 'admin', 'closed', 'awaiting_deposit', 'closed',
                         'admin_delisted', 101, at
                  FROM command_row, wall_clock
              )
         UPDATE public.consignment_listings
         SET state = 'closed', reason_code = 'admin_delisted', state_version = 4,
             updated_at = at
         FROM wall_clock
         WHERE id = $1 AND state = 'awaiting_deposit' AND state_version = 3",
    )
    .bind(Uuid::parse_str(super::support::LISTING).expect("挂单"))
    .bind(Uuid::new_v4())
    .execute(pool)
    .await
    .expect("关闭挂单");

    // 投递本身已收口，存量只能来自“已起写但尚无证明”。
    sqlx::query(
        "UPDATE public.deliveries
         SET status = 'failed', completed_at = clock_timestamp(),
             failure_code = 'fake_closed_work'
         WHERE id = $1",
    )
    .bind(delivery_id)
    .execute(pool)
    .await
    .expect("收口投递");
    assert!(!store.has_consignment_work().await.expect("仅剩收口投递时无存量"));

    let attempt_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.mail_disposition_attempts
            (id, work_id, delivery_id, fence, credential_id, credential_version,
             runtime_generation, session_id, expected_digest, prepared_at)
         VALUES ($1, $2, $3, 1, $4, 1, 1, $5, decode(repeat('a6', 32), 'hex'),
                 clock_timestamp())",
    )
    .bind(attempt_id)
    .bind(work_id)
    .bind(delivery_id)
    .bind(Uuid::parse_str(super::support::CREDENTIAL).expect("凭据"))
    .bind(Uuid::parse_str(super::support::SESSION).expect("会话"))
    .execute(pool)
    .await
    .expect("写入 attempt");
    sqlx::query(
        "INSERT INTO public.mail_disposition_write_starts
            (work_id, decision_id, delivery_id, attempt_id, fence, started_at, command_id)
         VALUES ($1, $2, $3, $4, 1, clock_timestamp(), $5)",
    )
    .bind(work_id)
    .bind(decision_id)
    .bind(delivery_id)
    .bind(attempt_id)
    .bind(decision_command.as_uuid())
    .execute(pool)
    .await
    .expect("写入起写事实");
    assert!(store.has_consignment_work().await.expect("已起写未收口是存量"));

    // 补上证明即全部收口，存量消失。
    sqlx::query(
        "INSERT INTO public.mail_disposition_proofs
            (id, work_id, decision_id, delivery_id, fence, attempt_id, kind, asset_leg,
             observation_id, validator_version, digest, conclusion, proven_at)
         VALUES ($1, $2, $3, $4, 1, $5, 'exact_return', 'original', $6, 'v1',
                 decode(repeat('a7', 32), 'hex'), 'return_proven', clock_timestamp())",
    )
    .bind(Uuid::new_v4())
    .bind(work_id)
    .bind(decision_id)
    .bind(delivery_id)
    .bind(attempt_id)
    .bind(observation_id)
    .execute(pool)
    .await
    .expect("写入证明");
    assert!(!store.has_consignment_work().await.expect("全部收口后无存量"));
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.mail_quarantine_rechecks WHERE state IN ('queued','leased')",
    )
    .fetch_one(pool)
    .await
    .expect("读取重检工作");
    assert_eq!(queued, 0);
}

#[tokio::test]
async fn postgres_closed_consignment_queued_recheck_is_work() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号");
    let pool = store.postgres_pool();
    assert!(!store.has_consignment_work().await.expect("空库存量"));
    seed_base(pool).await;
    let repository = repository(&database).await;
    let obligation = Uuid::parse_str("10000000-0000-0000-0000-000000000098").expect("义务");
    let subject = format!("[RTS] Auction equipment delivery [{obligation}]");
    let observation = observation(obligation, subject.clone(), Vec::new());
    insert_observation(pool, &observation, obligation, &subject).await;
    repository
        .decide_envelope(decide_quarantine(observation, "closed-work-recheck"))
        .await
        .expect("首次隔离");
    // 测试只保留 queued 重检作为存量，避免活动预约本身让启动条件假阳性；删除投影由维护账号
    // 在夹具中完成，不改变生产路径。
    let maintenance = database.maintenance_store().await.expect("维护账号");
    sqlx::query(
        "DELETE FROM public.consignment_equipment_claims
         WHERE reservation_id = $1",
    )
    .bind(Uuid::parse_str(super::support::RESERVATION).expect("预约"))
    .execute(maintenance.postgres_pool())
    .await
    .expect("删除测试占用");
    sqlx::query("DELETE FROM public.consignment_deposit_reservations WHERE id = $1")
        .bind(Uuid::parse_str(super::support::RESERVATION).expect("预约"))
        .execute(maintenance.postgres_pool())
        .await
        .expect("删除测试预约");
    sqlx::query(
        "WITH wall_clock AS MATERIALIZED (SELECT clock_timestamp() AS at),
              command_row AS (
                  INSERT INTO public.consignment_commands
                      (command_id, actor_kind, actor_ref, operation, request_target,
                       request_digest, replay_key_id, listing_id, result_status, result_code,
                       result_body, created_at)
                  SELECT $2, 'admin', 'fake-admin', 'admin_delist', $1,
                         decode(repeat('c1', 32), 'hex'), NULL, $1, 200, 'ok',
                         jsonb_build_object(
                             'command_id', $2,
                             'operation', 'admin_delist',
                             'listing_id', $1,
                             'listing_version', 4,
                             'state', 'closed',
                             'disposition', 'closed',
                             'price', 101),
                         at
                  FROM wall_clock
                  RETURNING command_id
              ),
              event_row AS (
                  INSERT INTO public.consignment_events
                      (listing_id, state_version, command_id, actor_kind, event_kind,
                       from_state, to_state, reason_code, price, occurred_at)
                  SELECT $1, 4, command_id, 'admin', 'closed', 'awaiting_deposit', 'closed',
                         'admin_delisted', 101, at
                  FROM command_row, wall_clock
              )
         UPDATE public.consignment_listings
         SET state = 'closed', reason_code = 'admin_delisted', state_version = 4,
             updated_at = at
         FROM wall_clock
         WHERE id = $1 AND state = 'awaiting_deposit' AND state_version = 3",
    )
    .bind(Uuid::parse_str(super::support::LISTING).expect("挂单"))
    .bind(Uuid::new_v4())
    .execute(pool)
    .await
    .expect("关闭挂单");
    assert!(store.has_consignment_work().await.expect("关闭后的存量"));
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.mail_quarantine_rechecks WHERE state = 'queued'",
    )
    .fetch_one(pool)
    .await
    .expect("读取重检工作");
    assert_eq!(queued, 1);
}
