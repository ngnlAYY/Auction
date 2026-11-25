//! 来源守卫（5C）：寄售义务不被拍卖 owner 判定、不被拍卖退件案件绑定、不被通用领取捡走。
//!
//! 保护不变量 6：义务来源不明或不是本 owner 时失败关闭；寄售退件不能被吞成拍卖案件。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use auction_application::consignment::LegacyAuctionOwnerResolver;
use auction_application::recovery::RecoveryWorkStore;
use auction_application::settlement::SettlementFulfillmentStore;
use auction_domain::SettlementWorld;
use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::deposit::{EnvelopeObservation, SenderIdentity};
use auction_storage::{
    DepositRepository, DepositSettings, ReservationSecretAad, ReservationSecretCrypto,
    ReservationSecretCryptoError, SealedReservationSecret,
};
use auction_time::{Duration, Timestamp};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::deposit::support::{DECIDE, SESSION};
use super::fixture::{Graph, OBLIGATION_1, OBLIGATION_3, seed_main_custody};
use crate::settlement_returns::{evidence, insert_inbound};
use crate::support::TestDatabase;

const BOT_UID: &str = "12345";

fn observation_id(index: u128) -> Uuid {
    Uuid::from_u128(0x8f00_0000_0000_0000_0000_0000_0000_1000 + index)
}

/// 写入一条系统退件观察并返回判定入参；`legacy_subject` 是旧证据里记录的原主题。
pub(in crate::consignment) async fn insert_system_return(
    pool: &PgPool,
    index: u128,
    obligation_id: &str,
    subject: &str,
) -> (OriginalMailKey, EnvelopeObservation) {
    let mid = format!("70{index}");
    let observation = EnvelopeObservation {
        observation_id: observation_id(index),
        session_binding_digest: [0x33; 32],
        manifest_digest: [0x6d; 32],
        parser_version: "parser-legacy".to_owned(),
        subject_is_consign: false,
        sender_identity: SenderIdentity::Verified,
        list_detail_consistent: true,
        asset_count: Some(0),
        duplicate_raw_eid: false,
        canonical_alias_ambiguous: false,
        unknown_attachment: false,
        credits: Some(0),
        cod: Some(0),
        returnable: false,
        takeable: Some(false),
        system_rts: true,
        legacy_auction_owner: None,
        items: Vec::new(),
    };
    sqlx::query(
        "INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
             evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
             returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
             season_window_id)
         VALUES ($1, $2, 'main', $3, 'source-guard', $4, 'fake-mailbox', NULL, NULL,
                 '2026-01-01T04:00:00Z', 'parser-legacy', NULL,
                 decode(repeat('72', 32), 'hex'), decode(repeat('73', 32), 'hex'), $5,
                 'MoogleMail', 'FakeBot', false, false, 0, 0, NULL, '[]'::jsonb, $6, NULL)",
    )
    .bind(observation.observation_id)
    .bind(BOT_UID)
    .bind(&mid)
    .bind(Uuid::parse_str(SESSION).unwrap())
    .bind(observation.manifest_digest.as_slice())
    .bind(json!({
        "subject_is_consign": false,
        "sender_identity": "verified",
        "list_detail_consistent": true,
        "duplicate_raw_eid": false,
        "canonical_alias_ambiguous": false,
        "unknown_attachment": false,
        "system_rts": true,
        "legacy_auction_owner": null,
        "legacy_obligation_id": obligation_id,
        "legacy_subject": subject,
        "returned_from": null,
        "body_digest": null,
        "dispatch_reference": null,
        "received_minute_raw": "2026-01-01T03:00",
        "asset_count": 0,
        "items": [],
    }))
    .execute(pool)
    .await
    .expect("写入系统退件观察");
    (OriginalMailKey::new(BOT_UID, SettlementWorld::Main, &mid).unwrap(), observation)
}

/// 交接原语不涉及预约密钥：任何加解密调用都失败。
struct NoCrypto;

impl ReservationSecretCrypto for NoCrypto {
    fn seal(
        &self,
        _token: &auction_domain::consignment::deposit::TokenText,
        _aad: ReservationSecretAad,
    ) -> Result<SealedReservationSecret, ReservationSecretCryptoError> {
        Err(ReservationSecretCryptoError)
    }

    fn open(
        &self,
        _sealed: &SealedReservationSecret,
        _aad: ReservationSecretAad,
    ) -> Result<auction_domain::consignment::deposit::TokenText, ReservationSecretCryptoError> {
        Err(ReservationSecretCryptoError)
    }
}

async fn count(pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.expect("计数")
}

/// 寄售义务 ID 不会被拍卖 owner 判定接收：寄售规范主题、以及套着拍卖主题的寄售义务 ID
/// 都返回“非拍卖”，且不产生案件与交接行。
#[tokio::test]
async fn postgres_consignment_return_is_not_judged_to_an_auction_owner() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(Graph::sale_main().sql(DECIDE)))
        .execute(pool)
        .await
        .expect("合同图");
    let store = database.store().await.expect("运行账号");

    let subjects = [
        format!("[RTS] Consignment buyer refund [{OBLIGATION_3}]"),
        // 寄售义务 ID 出现在拍卖 owner 的反查里：库里查得到，但来源是寄售，必须返回“非拍卖”。
        format!("[RTS] Auction buyer refund [{OBLIGATION_3}]"),
    ];
    for (index, subject) in subjects.iter().enumerate() {
        let (key, observation) =
            insert_system_return(pool, u128::try_from(index).unwrap() + 1, OBLIGATION_3, subject)
                .await;
        let resolved = store.resolve(&key, &observation).await.expect("旧 owner 判定");
        assert_eq!(resolved, None, "{subject}");
    }
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_cases").await, 0);
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_handoffs").await,
        0
    );
}

/// 拍卖退件消费不接触寄售义务：不领取、不建案件、不写交接行，寄售履约行不被转人工；
/// 直接走事务内入口时失败关闭。
#[tokio::test]
async fn postgres_consignment_obligation_is_not_bound_by_an_auction_return_case() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(Graph::sale_main().sql(DECIDE)))
        .execute(pool)
        .await
        .expect("合同图");
    let store = database.store().await.expect("运行账号");

    let received_at = Timestamp::parse_from_rfc3339("2026-01-03T00:00:00Z").unwrap();
    let mail = insert_inbound(
        pool,
        "consignment-guard-1",
        received_at,
        Some(("MoogleMail", format!("[RTS] Auction buyer refund [{OBLIGATION_3}]"))),
        evidence(Uuid::parse_str(OBLIGATION_3).unwrap(), 1_000, &[]),
    )
    .await;
    let consumed = store
        .consume_due_settlement_return(received_at + Duration::hours(1))
        .await
        .expect("消费退件");
    assert!(consumed.is_none(), "寄售义务的退件不得被拍卖消费者领取：{mail}");
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_cases").await, 0);
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_handoffs").await,
        0
    );
    let bound: i64 = count(
        pool,
        "SELECT count(*) FROM public.settlement_return_evidence WHERE obligation_id IS NOT NULL",
    )
    .await;
    assert_eq!(bound, 0);
    // 纵深防御：绕过领取队列、直接走事务内入口时同样失败关闭，不写证据、案件与交接行。
    let (_, observation) = insert_system_return(
        pool,
        9,
        OBLIGATION_3,
        &format!("[RTS] Auction buyer refund [{OBLIGATION_3}]"),
    )
    .await;
    let direct_mail = insert_inbound(
        pool,
        "709",
        received_at,
        Some(("MoogleMail", format!("[RTS] Auction buyer refund [{OBLIGATION_3}]"))),
        evidence(Uuid::parse_str(OBLIGATION_3).unwrap(), 1_000, &[]),
    )
    .await;
    let repo = DepositRepository::new(
        store.clone(),
        Arc::new(NoCrypto),
        DepositSettings::new(
            auction_domain::consignment::FeeBps::new(500).unwrap(),
            auction_domain::consignment::ListingPrice::new(1).unwrap(),
            auction_domain::consignment::ListingPrice::new(1_000_000_000).unwrap(),
            24,
            100,
        ),
    );
    let refused = repo.test_handoff_return_case(direct_mail, observation.observation_id).await;
    assert!(refused.is_err(), "寄售义务不得经拍卖原案件入口建案：{refused:?}");
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_cases").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_evidence").await, 0);
    let states: Vec<String> = sqlx::query_scalar(
        "SELECT execution_state FROM public.settlement_obligation_fulfillments
         WHERE obligation_id IN (SELECT id FROM public.settlement_obligations
                                 WHERE contract_id IS NOT NULL)",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert!(states.iter().all(|state| state == "pending"), "{states:?}");
}

/// 通用领取对寄售来源零领取：就绪的寄售义务不被拍卖履约 Worker 捡走，也没有任何投递。
#[tokio::test]
async fn postgres_generic_fulfillment_claim_ignores_consignment_obligations() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(Graph::sale_main().sql(DECIDE)))
        .execute(pool)
        .await
        .expect("合同图");
    let ready: String = sqlx::query_scalar(
        "SELECT business_state FROM public.settlement_obligations WHERE id = $1",
    )
    .bind(Uuid::parse_str(OBLIGATION_1).unwrap())
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(ready, "ready", "夹具必须含就绪的寄售义务，否则本例不证明任何事");
    let store = database.store().await.expect("运行账号");

    let now = Timestamp::parse_from_rfc3339("2026-01-03T00:00:00Z").unwrap();
    let claims = store
        .claim_due("source-guard-worker".to_owned(), now, now + Duration::minutes(5), 10)
        .await
        .expect("通用领取");
    assert!(claims.is_empty(), "{claims:?}");
    // 夹具自带寄售收货投递（origin_kind 不是 settlement）；结算来源的投递必须为零。
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.deliveries WHERE origin_kind = 'settlement'")
            .await,
        0
    );
    let claimed: i64 = count(
        pool,
        "SELECT count(*) FROM public.settlement_obligation_fulfillments
         WHERE claim_token IS NOT NULL",
    )
    .await;
    assert_eq!(claimed, 0);
}
