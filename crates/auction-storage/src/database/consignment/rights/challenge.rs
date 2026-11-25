//! 找回、发码与核验：挑战行、失败尝试计数与会话授予。
//!
//! 保护的不变量：
//! - 1：四个命令都先过账本仲裁；同 ID 同请求原样重放，核验重放先用当前密钥重算 MAC 比对，
//!   相符才只轮换令牌、沿用原截止；密钥轮换后重放失败关闭为 `ChallengeInvalid`。
//! - 2、6：失败尝试是追加行（`c7_attempt_max_ck` 限 5 次），第 5 次失败后挑战作废，第 6 次即使码对也拒；
//!   消费至多一次（`c7_attempt_consumed_uq`）。过期、已消费、用尽、虚拟、未匹配、浏览器不符、权利版本
//!   已变化一律同形 `ChallengeInvalid`，且核验无论目标真假都计算一次 MAC。
//! - 7：挑战码只以 MAC 入库、密文入 outbox；真实、虚拟与被节流的发码共用 [`issue`] 的同一段写路径，
//!   虚拟与被节流只是 outbox 行不写。

use std::net::IpAddr;

use auction_application::consignment::{
    BrowserBinding, ChallengeAad, ChallengeIssued, ChallengeRightCommand, LocateRightCommand,
    RightChallengeCrypto, RightVerified, RightsStoreError, VerifyRightCommand,
};
use auction_domain::consignment::rights::{
    CHALLENGE_CODE_ENTROPY_BYTES, CHALLENGE_TTL, ChallengeCode, ChallengeState, MAX_FAILED_ATTEMPTS,
};
use auction_domain::consignment::settlement_world_code;
use auction_time::Timestamp;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, Row};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use super::ledger::{self, LedgerKey, StoredReceipt, Subject};
use super::outbox::{self, NoticeRow};
use super::rights::{self, RightRow, identity_digest};
use super::session;
use super::{Persisted, PgRightsStore, budget, lock_key, random_bytes};

/// 发码请求的目标查找方式。
#[derive(Clone, Copy)]
enum Lookup<'a> {
    /// 找回：按世界与冻结身份文本。
    Identity { world: &'static str, name: &'a str },
    /// 发码：按定位符摘要。
    Locator([u8; 32]),
}

/// 一次发码请求的公共输入。
struct IssueRequest<'a> {
    key: LedgerKey,
    browser: BrowserBinding,
    ip: IpAddr,
    lookup: Lookup<'a>,
    notice_kind: &'static str,
}

impl PgRightsStore {
    /// 找回：按冻结身份发一封携带挑战码的通知；返回与目标是否真实无关的同形受理结果。
    pub(super) async fn locate_right(
        &self,
        command: &LocateRightCommand,
    ) -> Result<ChallengeIssued, RightsStoreError> {
        let world = settlement_world_code(command.world);
        let request = IssueRequest {
            key: browser_key(
                command.command_id.as_uuid(),
                command.browser,
                "locate_right",
                command.request_hash.as_bytes(),
            ),
            browser: command.browser,
            ip: command.client_ip,
            lookup: Lookup::Identity { world, name: command.game_identity.as_str() },
            notice_kind: "locator_guide",
        };
        let throttle = identity_digest(world, command.game_identity.as_str());
        self.issue_challenge(&request, &throttle).await
    }

    /// 发码：按定位符摘要发一封携带挑战码的通知；未匹配的定位符同形。
    pub(super) async fn challenge_right(
        &self,
        command: &ChallengeRightCommand,
    ) -> Result<ChallengeIssued, RightsStoreError> {
        let digest = command.locator_digest.as_bytes();
        // 权利身份与定位符摘要不可变，事务外预读只用来决定要取哪把身份锁；事务内重新读取。
        let throttle = self
            .read(&digest, |digest, connection, _now| {
                Box::pin(async move { rights::by_locator(connection, *digest).await })
            })
            .await?
            .map_or(digest, |row| row.identity_digest());
        let request = IssueRequest {
            key: browser_key(
                command.command_id.as_uuid(),
                command.browser,
                "challenge_right",
                command.request_hash.as_bytes(),
            ),
            browser: command.browser,
            ip: command.client_ip,
            lookup: Lookup::Locator(digest),
            notice_kind: "challenge",
        };
        self.issue_challenge(&request, &throttle).await
    }

    async fn issue_challenge(
        &self,
        request: &IssueRequest<'_>,
        throttle: &[u8; 32],
    ) -> Result<ChallengeIssued, RightsStoreError> {
        let locks = vec![
            lock_key("command", request.key.command_id.as_bytes()),
            lock_key("browser", &request.browser.0),
            lock_key("ip", &budget::ip_digest(request.ip)),
            lock_key("notice-identity", throttle),
        ];
        self.transact(locks, &(self, request), |(store, request), connection, now| {
            Box::pin(issue(store, connection, now, request))
        })
        .await
    }

    /// 核验：虚拟、错误、过期、超次、浏览器不符统一 `ChallengeInvalid`；成功发单权利会话。
    pub(super) async fn verify_right(
        &self,
        command: &VerifyRightCommand,
    ) -> Result<RightVerified, RightsStoreError> {
        let key = browser_key(
            command.command_id.as_uuid(),
            command.browser,
            "verify_right",
            command.request_hash.as_bytes(),
        );
        let locks = vec![
            lock_key("command", key.command_id.as_bytes()),
            lock_key("challenge", command.challenge_id.as_bytes()),
        ];
        self.transact(locks, &(self, command, key), |(store, command, key), connection, now| {
            Box::pin(verify_in_tx(store, connection, now, command, *key))
        })
        .await
    }
}

fn browser_key(
    command_id: Uuid,
    browser: BrowserBinding,
    operation: &'static str,
    request_digest: [u8; 32],
) -> LedgerKey {
    LedgerKey { command_id, subject: Subject::Browser(browser.0), operation, request_digest }
}

fn replay_issued(receipt: &StoredReceipt) -> Persisted<ChallengeIssued> {
    if let Some(error) = receipt.rejection() {
        return Ok(Err(error));
    }
    Ok(Ok(ChallengeIssued {
        challenge_id: receipt.uuid("challenge_id")?,
        expires_at: receipt.timestamp("expires_at")?,
    }))
}

/// 发码的唯一写路径：预算、挑战行、（可选）outbox 行与回执同一事务。
async fn issue(
    store: &PgRightsStore,
    connection: &mut PgConnection,
    now: Timestamp,
    request: &IssueRequest<'_>,
) -> Persisted<ChallengeIssued> {
    if let Some(receipt) = ledger::arbitrate(connection, &request.key).await? {
        return replay_issued(&receipt);
    }
    budget::spend(connection, request.key.command_id, request.browser, request.ip, now).await?;
    let crypto = store.crypto()?;
    let target = match request.lookup {
        Lookup::Identity { world, name } => rights::by_identity(connection, world, name).await?,
        Lookup::Locator(digest) => rights::by_locator(connection, digest).await?,
    };
    let challenge_id = Uuid::new_v4();
    let expires_at = now.checked_add_signed(CHALLENGE_TTL).ok_or(RightsStoreError::Unavailable)?;
    // 无论目标真假都生成码、算 MAC、封装：耗时与是否真实无关，虚拟的结果直接丢弃。
    let code = ChallengeCode::from_entropy(random_bytes::<CHALLENGE_CODE_ENTROPY_BYTES>());
    let aad = target.as_ref().map_or(
        ChallengeAad { challenge_id, right_id: None, identity_digest: [0; 32], right_version: 0 },
        |right| ChallengeAad {
            challenge_id,
            right_id: Some(right.right_id),
            identity_digest: right.identity_digest(),
            right_version: right.version,
        },
    );
    let mac = crypto.mac(aad, &code)?;
    let sealed = crypto.seal(&code, aad)?;
    let (world, name) = match (&target, request.lookup) {
        (Some(right), _) => (settlement_world_code(right.world), right.frozen_identity.as_str()),
        (None, Lookup::Identity { world, name }) => (world, name),
        (None, Lookup::Locator(_)) => ("main", ""),
    };
    let slot_free = outbox::slot_free(connection, world, name, now).await?;
    // 只有真实且未被节流的目标才投递；其余写同样的挑战与回执，只是不写 outbox 行。
    let delivered = target.as_ref().filter(|_| slot_free);
    insert_challenge(
        connection,
        challenge_id,
        delivered,
        request.browser,
        &mac,
        &sealed,
        now,
        expires_at,
    )
    .await?;
    let notice = delivered.map(|right| NoticeRow {
        kind: request.notice_kind,
        challenge_id,
        right_id: right.right_id,
        world_id: settlement_world_code(right.world),
        frozen_identity: right.frozen_identity.clone(),
        sealed,
    });
    outbox::enqueue(connection, notice, now).await?;
    let receipt = ledger::body(
        &request.key,
        "request_received",
        json!({ "challenge_id": challenge_id, "expires_at": expires_at }),
    );
    ledger::record(connection, &request.key, 202, "ok", receipt, now).await?;
    Ok(Ok(ChallengeIssued { challenge_id, expires_at }))
}

#[allow(clippy::too_many_arguments, reason = "挑战行的列与写入参数一一对应")]
async fn insert_challenge(
    connection: &mut PgConnection,
    challenge_id: Uuid,
    delivered: Option<&RightRow>,
    browser: BrowserBinding,
    mac: &[u8; 32],
    sealed: &auction_application::consignment::SealedChallengeSecret,
    now: Timestamp,
    expires_at: Timestamp,
) -> Result<(), super::TxError> {
    let nonce_digest: [u8; 32] = Sha256::digest(sealed.nonce()).into();
    let version = delivered.map(|right| i64::try_from(right.version)).transpose()?;
    sqlx::query(
        "INSERT INTO public.consignment_right_challenges
            (challenge_id, right_id, is_virtual, right_version, browser_binding_digest,
             code_mac, nonce_digest, identity_digest, created_at, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(challenge_id)
    .bind(delivered.map(|right| right.right_id))
    .bind(delivered.is_none())
    .bind(version)
    .bind(browser.0.as_slice())
    .bind(delivered.map(|_| mac.as_slice()))
    .bind(delivered.map(|_| nonce_digest.as_slice()))
    .bind(delivered.map(RightRow::identity_digest).as_ref().map(<[u8; 32]>::as_slice))
    .bind(now)
    .bind(expires_at)
    .execute(connection)
    .await?;
    Ok(())
}

/// 已持久化的挑战行。
struct ChallengeRow {
    right_id: Option<Uuid>,
    right_version: u64,
    browser_binding: Vec<u8>,
    code_mac: Option<Vec<u8>>,
    identity_digest: [u8; 32],
    expires_at: Timestamp,
}

/// 挑战行的列损坏：只记列名，不含值。
fn corrupt(column: &'static str) -> super::TxError {
    tracing::warn!(column, "受限权利挑战行数据损坏，失败关闭");
    super::TxError::Store(RightsStoreError::Unavailable)
}

async fn load_challenge(
    connection: &mut PgConnection,
    challenge_id: Uuid,
) -> Result<Option<ChallengeRow>, super::TxError> {
    let row = sqlx::query(
        "SELECT right_id, right_version, browser_binding_digest, code_mac, identity_digest, expires_at
         FROM public.consignment_right_challenges WHERE challenge_id = $1",
    )
    .bind(challenge_id)
    .fetch_optional(connection)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let identity: Option<Vec<u8>> = row.try_get("identity_digest")?;
    let version: Option<i64> = row.try_get("right_version")?;
    // 虚拟挑战的两列为空（零值陪跑）；有值却损坏则是库内数据错误，失败关闭而不是替换为零值。
    let identity_digest = match identity {
        Some(bytes) => {
            let Ok(digest) = <[u8; 32]>::try_from(bytes) else {
                return Err(corrupt("identity_digest"));
            };
            digest
        }
        None => [0; 32],
    };
    let right_version = match version {
        Some(value) => {
            let Ok(version) = u64::try_from(value) else {
                return Err(corrupt("right_version"));
            };
            version
        }
        None => 0,
    };
    Ok(Some(ChallengeRow {
        right_id: row.try_get("right_id")?,
        right_version,
        browser_binding: row.try_get("browser_binding_digest")?,
        code_mac: row.try_get("code_mac")?,
        identity_digest,
        expires_at: row.try_get("expires_at")?,
    }))
}

/// 用当前密钥重算码的 MAC 并常量时间比对；缺失行与虚拟行用零值陪跑，耗时与目标真假无关。
fn code_matches(
    crypto: &dyn RightChallengeCrypto,
    challenge_id: Uuid,
    challenge: Option<&ChallengeRow>,
    code: &ChallengeCode,
) -> Result<bool, super::TxError> {
    let aad = challenge.map_or(
        ChallengeAad { challenge_id, right_id: None, identity_digest: [0; 32], right_version: 0 },
        |row| ChallengeAad {
            challenge_id,
            right_id: row.right_id,
            identity_digest: row.identity_digest,
            right_version: row.right_version,
        },
    );
    let mac = crypto.mac(aad, code)?;
    let stored_mac = challenge.and_then(|row| row.code_mac.clone()).unwrap_or_else(|| vec![0; 32]);
    Ok(bool::from(mac.as_slice().ct_eq(stored_mac.as_slice())))
}

async fn verify_in_tx(
    store: &PgRightsStore,
    connection: &mut PgConnection,
    now: Timestamp,
    command: &VerifyRightCommand,
    key: LedgerKey,
) -> Persisted<RightVerified> {
    if let Some(receipt) = ledger::arbitrate(connection, &key).await? {
        if let Some(error) = receipt.rejection() {
            return Ok(Err(error));
        }
        // 同命令恢复同次授予：先用当前密钥重算码的 MAC 并与挑战行比对（密钥轮换后即不符，失败关闭、
        // 不换发令牌，不写账本），再只换令牌、沿用原截止；旧令牌即刻失效。
        let crypto = store.crypto()?;
        let challenge_id = receipt.uuid("challenge_id")?;
        let challenge = load_challenge(connection, challenge_id).await?;
        if !code_matches(crypto, challenge_id, challenge.as_ref(), &command.code)? {
            return Ok(Err(RightsStoreError::ChallengeInvalid));
        }
        let issued = session::rotate(connection, challenge_id, command.browser, now).await?;
        return Ok(Ok(RightVerified {
            right_id: receipt.uuid("right_id")?,
            right_version: receipt.version("right_version")?,
            expires_at: issued.expires_at,
            session_token: issued.token,
        }));
    }
    let crypto = store.crypto()?;
    let challenge = load_challenge(connection, command.challenge_id).await?;
    let (failed, consumed) = attempt_counts(connection, command.challenge_id).await?;
    // 不管目标真假都算一次 MAC，再用常量时间比较；虚拟与缺失行用零值陪跑。
    let code_ok = code_matches(crypto, command.challenge_id, challenge.as_ref(), &command.code)?;
    let binding_ok = challenge.as_ref().is_some_and(|row| {
        bool::from(row.browser_binding.as_slice().ct_eq(command.browser.0.as_slice()))
    });
    let open = challenge.as_ref().is_some_and(|row| {
        ChallengeState { expires_at: row.expires_at, failed_attempts: failed, consumed }
            .ensure_open(now)
            .is_ok()
    });
    let right = match challenge.as_ref().and_then(|row| row.right_id) {
        Some(right_id) => rights::by_id(connection, right_id).await?,
        None => None,
    };
    let version_ok = matches!((&right, &challenge), (Some(right), Some(row)) if right.version == row.right_version);
    // 只有属于本浏览器、仍开放的真实挑战上的错码才计失败；其余原因不计，避免第三方消耗他人的次数。
    if open && binding_ok && right.is_some() && !code_ok {
        sqlx::query(
            "INSERT INTO public.consignment_right_challenge_attempts
                (challenge_id, attempt, outcome, occurred_at)
             VALUES ($1, $2, 'failed', $3)",
        )
        .bind(command.challenge_id)
        .bind(i16::from(failed) + 1)
        .bind(now)
        .execute(&mut *connection)
        .await?;
    }
    let (Some(right), true) = (right, code_ok && binding_ok && open && version_ok) else {
        let error = RightsStoreError::ChallengeInvalid;
        ledger::record_rejection(connection, &key, error, now).await?;
        return Ok(Err(error));
    };
    sqlx::query(
        "INSERT INTO public.consignment_right_challenge_attempts
            (challenge_id, attempt, outcome, occurred_at)
         VALUES ($1, $2, 'consumed', $3)",
    )
    .bind(command.challenge_id)
    .bind(i16::from(failed) + 1)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    let scope = if right.is_active() { right.allowed.clone() } else { Vec::new() };
    let issued = session::grant(
        connection,
        command.challenge_id,
        right.right_id,
        &scope,
        command.browser,
        now,
    )
    .await?;
    let receipt = ledger::body(
        &key,
        "right_verified",
        json!({
            "challenge_id": command.challenge_id,
            "right_id": right.right_id,
            "right_version": right.version,
            "expires_at": issued.expires_at,
        }),
    );
    ledger::record(connection, &key, 200, "ok", receipt, now).await?;
    Ok(Ok(RightVerified {
        right_id: right.right_id,
        right_version: right.version,
        expires_at: issued.expires_at,
        session_token: issued.token,
    }))
}

/// 已记失败次数与是否已消费。
async fn attempt_counts(
    connection: &mut PgConnection,
    challenge_id: Uuid,
) -> Result<(u8, bool), super::TxError> {
    let (failed, consumed): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE outcome = 'failed'),
                count(*) FILTER (WHERE outcome = 'consumed')
         FROM public.consignment_right_challenge_attempts WHERE challenge_id = $1",
    )
    .bind(challenge_id)
    .fetch_one(connection)
    .await?;
    let failed = u8::try_from(failed).unwrap_or(MAX_FAILED_ATTEMPTS).min(MAX_FAILED_ATTEMPTS);
    Ok((failed, consumed > 0))
}
