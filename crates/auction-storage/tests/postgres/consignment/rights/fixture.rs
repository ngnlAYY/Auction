//! 受限权利测试夹具：迁移 7M-2 落地前的账本与预算表垫片、权利种子、可控时钟与测试密码适配器。
//!
//! 垫片只在库里还没有对应约束 / 表时才生效：7M-2 放宽账本 CHECK、集成人加入预算表之后它们是空操作。

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use auction_application::consignment::{
    BrowserBinding, ChallengeAad, ChallengeRightCommand, GameIdentity, LocateRightCommand,
    RightChallengeCrypto, RightCryptoError, SealedChallengeSecret, VerifyRightCommand,
};
use auction_application::core::RequestHash;
use auction_domain::consignment::rights::{ChallengeCode, RightLocator, locator_digest};
use auction_domain::{CommandId, SettlementWorld};
use auction_storage::{DatabaseStore, PgRightNoticeStore, PgRightsStore};
use auction_time::Timestamp;
use sha2::{Digest, Sha256};
use sqlx::{AssertSqlSafe, PgPool};
use uuid::Uuid;

use super::super::contracts::fixture::{CONTRACT, Graph, seed_main_custody};
use super::super::deposit::support::{DECIDE, USER_2, succeeds};
use crate::support::TestDatabase;

pub(super) const IDENTITY: &str = "FakeSellerTwo";
/// 测试时钟起点：2026-01-03T01:00:00Z。
const CLOCK_START: i64 = 1_767_402_000;

/// 测试密码适配器：以单字节“密钥”区分两把钥，模拟轮换。
pub(super) struct TestCrypto {
    key: u8,
}

impl TestCrypto {
    pub(super) fn new(key: u8) -> Arc<Self> {
        Arc::new(Self { key })
    }

    fn material(&self, label: &[u8], aad: ChallengeAad) -> Sha256 {
        let mut hash = Sha256::new();
        hash.update(label);
        hash.update([self.key]);
        hash.update(aad.challenge_id.as_bytes());
        hash.update(aad.right_id.map_or([0; 16], |id| *id.as_bytes()));
        hash.update(aad.identity_digest);
        hash.update(aad.right_version.to_be_bytes());
        hash
    }

    fn fingerprint(&self, code: &ChallengeCode) -> String {
        let mut hash = Sha256::new();
        hash.update(b"fingerprint");
        hash.update([self.key]);
        hash.update(code.as_bytes());
        hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn keystream(&self, aad: ChallengeAad, nonce: &[u8; 12]) -> [u8; 32] {
        let mut hash = self.material(b"seal", aad);
        hash.update(nonce);
        hash.finalize().into()
    }
}

impl RightChallengeCrypto for TestCrypto {
    fn seal(
        &self,
        code: &ChallengeCode,
        aad: ChallengeAad,
    ) -> Result<SealedChallengeSecret, RightCryptoError> {
        let mut nonce = [0_u8; 12];
        nonce.copy_from_slice(&Uuid::new_v4().as_bytes()[..12]);
        let stream = self.keystream(aad, &nonce);
        let ciphertext = code.as_bytes().iter().zip(stream).map(|(byte, key)| byte ^ key).collect();
        SealedChallengeSecret::new(ciphertext, nonce, "fixture-key".into(), self.fingerprint(code))
    }

    fn open(
        &self,
        sealed: &SealedChallengeSecret,
        aad: ChallengeAad,
    ) -> Result<ChallengeCode, RightCryptoError> {
        let stream = self.keystream(aad, sealed.nonce());
        let mut entropy = [0_u8; 16];
        for (slot, (byte, key)) in entropy.iter_mut().zip(sealed.ciphertext().iter().zip(stream)) {
            *slot = byte ^ key;
        }
        let code = ChallengeCode::from_entropy(entropy);
        if self.fingerprint(&code) != sealed.fingerprint() {
            return Err(RightCryptoError);
        }
        Ok(code)
    }

    fn mac(&self, aad: ChallengeAad, code: &ChallengeCode) -> Result<[u8; 32], RightCryptoError> {
        let mut hash = self.material(b"mac", aad);
        hash.update(code.as_bytes());
        Ok(hash.finalize().into())
    }
}

/// 已播种的权利。
pub(super) struct SeededRight {
    pub(super) right_id: Uuid,
    pub(super) locator: RightLocator,
}

/// 测试台：库、运行账号存储、维护账号连接池、可控时钟与密码适配器。
pub(super) struct Rig {
    _database: TestDatabase,
    pub(super) store: Arc<PgRightsStore>,
    pub(super) pool: PgPool,
    pub(super) crypto: Arc<TestCrypto>,
    clock: Arc<AtomicI64>,
    runtime: DatabaseStore,
    _maintenance: DatabaseStore,
}

impl Rig {
    pub(super) async fn new() -> Self {
        let database = TestDatabase::create().await.expect("克隆模板库");
        let maintenance = database.maintenance_store().await.expect("以维护账号连接");
        let pool = maintenance.postgres_pool().clone();
        seed_main_custody(&pool).await;
        succeeds(&pool, &Graph::sale_main().sql(DECIDE)).await;
        let runtime = database.store().await.expect("以运行账号连接");
        let clock = Arc::new(AtomicI64::new(CLOCK_START));
        let crypto = TestCrypto::new(1);
        let store = Arc::new(build_store(&runtime, Some(crypto.clone()), &clock));
        Self { _database: database, store, pool, crypto, clock, runtime, _maintenance: maintenance }
    }

    /// 运行账号连接池（密钥轮换以运行账号改写密文列）。
    pub(super) fn runtime_pool(&self) -> &PgPool {
        self.runtime.postgres_pool()
    }

    /// 同库同时钟的权利通知工作存储（7D-5）。
    pub(super) fn notice_store(&self) -> PgRightNoticeStore {
        let clock = Arc::clone(&self.clock);
        PgRightNoticeStore::new(self.runtime.clone()).with_test_clock(Arc::new(move || {
            Timestamp::from_timestamp(clock.load(Ordering::SeqCst), 0).expect("时钟")
        }))
    }

    /// 同库同时钟、另配密码适配器的存储（密钥轮换、未注入）。
    pub(super) fn store_with(&self, crypto: Option<Arc<TestCrypto>>) -> PgRightsStore {
        build_store(&self.runtime, crypto, &self.clock)
    }

    pub(super) fn advance_minutes(&self, minutes: i64) {
        self.clock.fetch_add(minutes * 60, Ordering::SeqCst);
    }

    /// 在共享合同上登记一项权利（定位符摘要由 `n` 决定）。
    pub(super) async fn seed_right(&self, n: u8, kind: &str, allowed: &[&str]) -> SeededRight {
        let right_id = Uuid::from_u128(0x7c10_0000_0000_4000_8000_0000_0000_0000 + u128::from(n));
        let locator = RightLocator::from_entropy([n; 32]);
        let command: Uuid = sqlx::query_scalar(
            "SELECT command_id FROM public.consignment_commands ORDER BY created_at, command_id LIMIT 1",
        )
        .fetch_one(&self.pool)
        .await
        .expect("夹具应提供一条命令");
        let beneficiary = Uuid::parse_str(USER_2).expect("用户");
        let contract = Uuid::parse_str(CONTRACT).expect("合同");
        sqlx::query(
            "INSERT INTO public.consignment_rights
                (right_id, locator_digest, beneficiary_user_id, frozen_identity, world_id, kind,
                 contract_id, position_id, created_command_id, created_at)
             VALUES ($1, $2, $3, $4, 'main', $5, $6, NULL, $7, '2026-01-03T00:00:00Z')",
        )
        .bind(right_id)
        .bind(locator_digest(&locator).as_bytes().as_slice())
        .bind(beneficiary)
        .bind(IDENTITY)
        .bind(kind)
        .bind(contract)
        .bind(command)
        .execute(&self.pool)
        .await
        .expect("登记权利");
        let allowed: Vec<String> = allowed.iter().map(|action| (*action).to_owned()).collect();
        sqlx::query(
            "INSERT INTO public.consignment_right_states (right_id, version, state, allowed_actions, occurred_at)
             VALUES ($1, 1, 'active', $2, '2026-01-03T00:00:00Z')",
        )
        .bind(right_id)
        .bind(&allowed)
        .execute(&self.pool)
        .await
        .expect("登记权利状态");
        SeededRight { right_id, locator }
    }

    pub(super) async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(AssertSqlSafe(sql.to_owned())).fetch_one(&self.pool).await.expect("计数")
    }

    /// 从专用 outbox 解封某挑战的码（测试替代投递）。
    pub(super) async fn open_code(&self, challenge_id: Uuid) -> ChallengeCode {
        let (payload, nonce, key_id, right_id, identity, version): (
            Vec<u8>,
            Vec<u8>,
            String,
            Uuid,
            Vec<u8>,
            i64,
        ) = sqlx::query_as(
            "SELECT o.sealed_payload, o.nonce, o.encryption_key_id, c.right_id, c.identity_digest,
                    c.right_version
             FROM public.consignment_right_notice_outbox AS o
             JOIN public.consignment_right_challenges AS c USING (challenge_id)
             WHERE o.challenge_id = $1",
        )
        .bind(challenge_id)
        .fetch_one(&self.pool)
        .await
        .expect("该挑战应有 outbox 行");
        let split = payload.len() - 64;
        let sealed = SealedChallengeSecret::new(
            payload[..split].to_vec(),
            <[u8; 12]>::try_from(nonce).expect("nonce"),
            key_id,
            String::from_utf8(payload[split..].to_vec()).expect("指纹"),
        )
        .expect("密文形状");
        let aad = ChallengeAad {
            challenge_id,
            right_id: Some(right_id),
            identity_digest: <[u8; 32]>::try_from(identity).expect("身份摘要"),
            right_version: u64::try_from(version).expect("版本"),
        };
        self.crypto.open(&sealed, aad).expect("解封挑战码")
    }
}

fn build_store(
    runtime: &DatabaseStore,
    crypto: Option<Arc<TestCrypto>>,
    clock: &Arc<AtomicI64>,
) -> PgRightsStore {
    let clock = Arc::clone(clock);
    PgRightsStore::new(
        runtime.clone(),
        crypto.map(|crypto| crypto as Arc<dyn RightChallengeCrypto>),
    )
    .with_test_clock(Arc::new(move || {
        Timestamp::from_timestamp(clock.load(Ordering::SeqCst), 0).expect("时钟")
    }))
}

pub(super) fn browser(byte: u8) -> BrowserBinding {
    BrowserBinding([byte; 32])
}

fn request_hash(seed: Uuid) -> RequestHash {
    RequestHash::new(Sha256::digest(seed.as_bytes()).into())
}

pub(super) fn locate_command(id: Uuid, byte: u8, ip: &str, identity: &str) -> LocateRightCommand {
    LocateRightCommand {
        command_id: CommandId::from_uuid(id),
        browser: browser(byte),
        client_ip: ip.parse::<IpAddr>().expect("IP"),
        game_identity: GameIdentity::parse(identity).expect("身份"),
        world: SettlementWorld::Main,
        request_hash: request_hash(id),
    }
}

pub(super) fn challenge_command(
    id: Uuid,
    byte: u8,
    ip: &str,
    right: &SeededRight,
) -> ChallengeRightCommand {
    ChallengeRightCommand {
        command_id: CommandId::from_uuid(id),
        browser: browser(byte),
        client_ip: ip.parse::<IpAddr>().expect("IP"),
        locator_digest: locator_digest(&right.locator),
        request_hash: request_hash(id),
    }
}

pub(super) fn verify_command(
    id: Uuid,
    byte: u8,
    challenge_id: Uuid,
    code: ChallengeCode,
) -> VerifyRightCommand {
    VerifyRightCommand {
        command_id: CommandId::from_uuid(id),
        browser: browser(byte),
        challenge_id,
        code,
        request_hash: request_hash(id),
    }
}
