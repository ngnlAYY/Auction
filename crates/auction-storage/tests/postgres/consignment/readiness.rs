//! 收件证据会话、世界就绪与旧退件派生事实的 PG18 回归。

use auction_application::consignment::{
    ConsignmentError, MailEvidenceReadinessRecord, MailEvidenceReadinessStore,
    MailEvidenceSessionDraft, MailReadinessPolicy,
};
use auction_domain::SettlementWorld;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::support::TestDatabase;

const CREDENTIAL: Uuid = Uuid::from_u128(0x8100_0000_0000_0000_0000_0000_0000_0001);
const SESSION: Uuid = Uuid::from_u128(0x8100_0000_0000_0000_0000_0000_0000_0002);
const OBSERVATION: Uuid = Uuid::from_u128(0x8400_0000_0000_0000_0000_0000_0000_0001);
const SECOND_OBSERVATION: Uuid = Uuid::from_u128(0x8400_0000_0000_0000_0000_0000_0000_0002);
const OBLIGATION: &str = "5b1c2a9e-1f0d-4c6b-9a7e-3d2f1e0c9b8a";

async fn seed_session(pool: &PgPool) {
    sqlx::query(
        "INSERT INTO public.credential_keys
            (id, credential_kind, scope, label, ciphertext, nonce, encryption_key_id,
             fingerprint, status, state_version, created_at, updated_at)
         VALUES ($1, 'mail_cookie', 'main', 'fake', decode('11', 'hex'),
                 decode(repeat('22', 12), 'hex'), 'fake-data-key', repeat('a', 64),
                 'active', 1, clock_timestamp(), clock_timestamp())",
    )
    .bind(CREDENTIAL)
    .execute(pool)
    .await
    .expect("假邮件凭据");
    sqlx::query(
        "INSERT INTO public.mail_evidence_sessions
            (id, bot_uid, world_id, player_name, credential_id, credential_version,
             runtime_generation, observed_at, protocol_version, binding_digest)
         VALUES ($1, '12345', 'main', 'FakeBot', $2, 1, 1,
                 clock_timestamp(), 'v1', decode(repeat('33', 32), 'hex'))",
    )
    .bind(SESSION)
    .bind(CREDENTIAL)
    .execute(pool)
    .await
    .expect("假证据会话");
}

fn readiness_record(service_epoch: u64) -> MailEvidenceReadinessRecord {
    MailEvidenceReadinessRecord {
        session: MailEvidenceSessionDraft {
            session_id: SESSION,
            world: SettlementWorld::Main,
            bot_uid: "12345".to_owned(),
            player_name: "FakeBot".to_owned(),
            credential_id: CREDENTIAL,
            credential_version: 1,
            runtime_generation: 1,
            protocol_version: "v1".to_owned(),
            binding_digest: [0x33; 32],
        },
        policy: MailReadinessPolicy {
            policy_digest: [0x44; 32],
            allowlist_mode: "enforced".to_owned(),
            allowed_senders: vec!["FakeSeller".to_owned()],
            validator_versions: vec!["parser-v1-unsigned".to_owned()],
            detail_ready: true,
            take_ready: false,
            return_ready: false,
        },
        service_epoch,
        valid_for_seconds: 30,
    }
}

#[tokio::test]
async fn postgres_mail_readiness_writes_session_then_world_and_rejects_missing_session() {
    let database = TestDatabase::create().await.expect("创建测试库");
    let owner = database.maintenance_store().await.expect("维护连接");
    let pool = owner.postgres_pool();
    seed_session(pool).await;
    let store = database.store().await.expect("运行账号连接");

    let first = store.record_mail_readiness(readiness_record(7)).await.expect("首次写就绪");
    assert_eq!(first.version, 1);
    assert!(first.valid_until > first.checked_at);
    let row: (String, bool, i64) = sqlx::query_as(
        "SELECT session_id::text, detail_ready, version
         FROM public.consignment_mail_readiness WHERE world_id = 'main'",
    )
    .fetch_one(pool)
    .await
    .expect("读回就绪");
    assert_eq!(row, (SESSION.to_string(), true, 1));

    let second = store.record_mail_readiness(readiness_record(8)).await.expect("续期就绪");
    assert_eq!(second.version, 2);
    assert!(second.checked_at >= first.checked_at);

    // 会话缺失时世界就绪的 FK 必须拒绝写入，不能留下没有证据会话的有效就绪。
    let error = sqlx::query(
        "INSERT INTO public.consignment_mail_readiness
            (world_id, session_id, service_epoch, policy_digest, allowlist_mode,
             allowed_senders, validator_versions, detail_ready, take_ready, return_ready,
             checked_at, valid_until, version)
         VALUES ('isekai', $1, 1, decode(repeat('44', 32), 'hex'), 'disabled',
                 ARRAY[]::text[], ARRAY[]::text[], true, false, false,
                 clock_timestamp(), clock_timestamp() + interval '30 seconds', 1)",
    )
    .bind(Uuid::new_v4())
    .execute(pool)
    .await
    .expect_err("缺失会话必须失败");
    let database_error = error.as_database_error().expect("服务端 SQLSTATE");
    assert_eq!(database_error.code().as_deref(), Some("23503"));
    assert_eq!(database_error.constraint(), Some("c4_readiness_session_fk"));
}

#[tokio::test]
async fn postgres_mail_readiness_rejects_session_reuse_with_different_identity() {
    let database = TestDatabase::create().await.expect("创建测试库");
    let owner = database.maintenance_store().await.expect("维护连接");
    let pool = owner.postgres_pool();
    seed_session(pool).await;
    let store = database.store().await.expect("运行账号连接");

    let first = store.record_mail_readiness(readiness_record(7)).await.expect("首次写就绪");
    assert_eq!(first.version, 1);

    // 同 ID 异内容按失败关闭处理：既不能复用原会话，也不能推进世界就绪版本。
    let mut changed_uid = readiness_record(8);
    changed_uid.session.bot_uid = "54321".to_owned();
    let uid_error = store
        .record_mail_readiness(changed_uid)
        .await
        .expect_err("同一会话 ID 的 bot_uid 冲突必须失败");
    assert!(matches!(uid_error, ConsignmentError::Unavailable));

    let mut changed_binding = readiness_record(9);
    changed_binding.session.binding_digest = [0x34; 32];
    let binding_error = store
        .record_mail_readiness(changed_binding)
        .await
        .expect_err("同一会话 ID 的绑定摘要冲突必须失败");
    assert!(matches!(binding_error, ConsignmentError::Unavailable));

    let version: i64 = sqlx::query_scalar(
        "SELECT version FROM public.consignment_mail_readiness WHERE world_id = 'main'",
    )
    .fetch_one(pool)
    .await
    .expect("读回就绪版本");
    assert_eq!(version, 1);
}

fn derived_facts(include_unknown: bool) -> serde_json::Value {
    let mut value = json!({
        "subject_is_consign": false,
        "sender_identity": "verified",
        "list_detail_consistent": true,
        "duplicate_raw_eid": false,
        "canonical_alias_ambiguous": false,
        "unknown_attachment": false,
        "system_rts": true,
        "legacy_auction_owner": "buyer_refund",
        "legacy_obligation_id": OBLIGATION,
        "returned_from": "Fake Buyer",
        "body_digest": "aa".repeat(32),
        "dispatch_reference": "v1/1/0d6c1d0c-63c4-5d0b-9a4f-1d7d2f8e9a01",
        "received_minute_raw": "2026-01-01 03:00",
        "asset_count": 0,
        "items": [],
    });
    if include_unknown {
        value["unknown_evidence_key"] = json!(true);
    }
    value
}

async fn insert_observation(
    pool: &PgPool,
    observation_id: Uuid,
    remote_observation_id: &str,
    derived_facts: serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, observed_at, parser_version, subject_digest, body_digest,
             manifest_digest, sender, recipient, returnable, takeable, credits, cod,
             raw_count, manifest, derived_facts, season_window_id)
         VALUES ($1, '12345', 'main', '555', $2, $3, 'fixture-mailbox',
                 clock_timestamp(), 'parser-v1-unsigned',
                 decode(repeat('55', 32), 'hex'), decode(repeat('66', 32), 'hex'),
                 decode(repeat('77', 32), 'hex'), 'MoogleMail', 'FakeBot', true, NULL,
                 0, 0, NULL, '[]'::jsonb, $4, NULL)",
    )
    .bind(observation_id)
    .bind(remote_observation_id)
    .bind(SESSION)
    .bind(derived_facts)
    .execute(pool)
    .await
    .map(|_| ())
}

#[tokio::test]
async fn postgres_observation_legacy_evidence_keys_are_writable_and_closed() {
    let database = TestDatabase::create().await.expect("创建测试库");
    let owner = database.maintenance_store().await.expect("维护连接");
    let pool = owner.postgres_pool();
    seed_session(pool).await;
    sqlx::query(
        "INSERT INTO public.mail_originals
            (bot_uid, world_id, external_mid, first_session_id, received_minute_raw,
             received_timezone, first_seen_at, season_window_id)
         VALUES ('12345', 'main', '555', $1, '2026-01-01 03:00', 'UTC',
                 clock_timestamp(), NULL)",
    )
    .bind(SESSION)
    .execute(pool)
    .await
    .expect("假原信");

    insert_observation(pool, OBSERVATION, "fixture-observation-1", derived_facts(false))
        .await
        .expect("旧退件证据应可写入");
    let stored: serde_json::Value = sqlx::query_scalar(
        "SELECT derived_facts FROM public.mail_envelope_observations WHERE id = $1",
    )
    .bind(OBSERVATION)
    .fetch_one(pool)
    .await
    .expect("读回派生事实");
    assert_eq!(stored["legacy_obligation_id"], json!(OBLIGATION));
    assert_eq!(stored["returned_from"], json!("Fake Buyer"));
    assert_eq!(stored["body_digest"], json!("aa".repeat(32)));

    let error =
        insert_observation(pool, SECOND_OBSERVATION, "fixture-observation-2", derived_facts(true))
            .await
            .expect_err("未知派生事实键必须失败");
    let database_error = error.as_database_error().expect("服务端 SQLSTATE");
    assert_eq!(database_error.code().as_deref(), Some("23514"));
    assert_eq!(database_error.constraint(), Some("c4_observation_derived_ck"));
}
