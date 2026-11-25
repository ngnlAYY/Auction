//! 购买资金预检的认证 Bot UID 来源：只取 main 收件就绪会话所绑定的当前 active 邮件凭据的 UID；
//! 凭据失效时读取失败（购买 503、不写回执），不伪造确定拒绝。以运行账号连接，同时证明运行账号能读
//! 凭据状态。

use auction_application::settlement::{
    ConsignmentFundingPreflight, FundingError, FundingPreflightFact,
};
use auction_domain::SettlementWorld;
use auction_domain::consignment::funding::FundingMode;
use auction_storage::ActiveBotFundingPreflight;
use sqlx::PgPool;
use uuid::Uuid;

use crate::support::TestDatabase;

const CREDENTIAL: Uuid = Uuid::from_u128(0x8200_0000_0000_0000_0000_0000_0000_0001);
const SESSION: Uuid = Uuid::from_u128(0x8200_0000_0000_0000_0000_0000_0000_0002);
const BOT_UID: &str = "7301";

/// 假 main 凭据、证据会话、世界就绪与一个 record_only 资金账户（无需 opening 即可就绪）。
async fn seed(pool: &PgPool) {
    for statement in [
        "INSERT INTO public.credential_keys
            (id, credential_kind, scope, label, ciphertext, nonce, encryption_key_id,
             fingerprint, status, state_version, created_at, updated_at)
         VALUES ($1, 'mail_cookie', 'main', 'fake', decode('11', 'hex'),
                 decode(repeat('22', 12), 'hex'), 'fake-data-key', repeat('a', 64),
                 'active', 1, clock_timestamp(), clock_timestamp())",
        "INSERT INTO public.mail_evidence_sessions
            (id, bot_uid, world_id, player_name, credential_id, credential_version,
             runtime_generation, observed_at, protocol_version, binding_digest)
         VALUES ($2, '7301', 'main', 'FakeBot', $1, 1, 1,
                 clock_timestamp(), 'v1', decode(repeat('33', 32), 'hex'))",
        "INSERT INTO public.consignment_mail_readiness
            (world_id, session_id, service_epoch, policy_digest, allowlist_mode,
             allowed_senders, validator_versions, detail_ready, take_ready, return_ready,
             checked_at, valid_until, version)
         VALUES ('main', $2, 1, decode(repeat('44', 32), 'hex'), 'disabled',
                 ARRAY[]::text[], ARRAY[]::text[], true, false, false,
                 clock_timestamp(), clock_timestamp() + interval '30 seconds', 1)",
        "INSERT INTO public.settlement_funding_accounts (uid, accounting_mode, status, state_version)
         VALUES ('7301', 'record_only', 'ready', 1)",
    ] {
        sqlx::query(statement)
            .bind(CREDENTIAL)
            .bind(SESSION)
            .execute(pool)
            .await
            .expect("预检夹具");
    }
}

#[tokio::test]
async fn postgres_consignment_preflight_uid_comes_from_the_active_main_mail_credential() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool: &PgPool = owner.postgres_pool();
    seed(pool).await;
    let preflight = ActiveBotFundingPreflight::new(database.store().await.expect("运行账号"));

    // 正例：isekai 购买同样以 main 认证 Bot 的 UID 预检（资金一律在 main）。
    let fact = preflight.preflight(SettlementWorld::Isekai).await.expect("预检");
    let FundingPreflightFact::Ready { uid, mode, .. } = fact else {
        panic!("预检应就绪：{fact:?}");
    };
    assert_eq!((uid.as_str(), mode), (BOT_UID, FundingMode::RecordOnly));

    // 反例：凭据被停用后，会话不再代表当前 active 凭据，UID 来源缺失即读取失败，不给确定事实。
    sqlx::query(
        "UPDATE public.credential_keys SET status = 'disabled', updated_at = clock_timestamp()
         WHERE id = $1",
    )
    .bind(CREDENTIAL)
    .execute(pool)
    .await
    .expect("停用凭据");
    let error = preflight.preflight(SettlementWorld::Main).await.expect_err("凭据失效必须失败");
    assert!(matches!(error, FundingError::Unavailable), "{error:?}");
}
