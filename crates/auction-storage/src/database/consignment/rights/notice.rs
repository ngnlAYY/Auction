//! 权利通知 outbox 的工作存储：领取、完成、释放。
//!
//! 不变量 4、5、6、7：
//! - 发送由 Worker 在本地提交之后执行；这里只维护持久租约（`claim_token` / 单调 `claim_fence` /
//!   `lease_expires_at`）与追加式结果。内存定时器只用来唤醒，到期与完成状态以本表为准。
//! - 租约行只有租约三列可变，已有结果的工作库内触发器拒绝再领取；`complete` 与 `release` 以令牌加
//!   围栏守卫，旧围栏的写入一律 `LeaseLost`。
//! - 租约过期后重新领取的工作带 `reclaimed`：上一次持有者可能已发出，只能记 `unknown`，不得重发。
//! - 只向冻结身份派发：收件人取自入队时冻结的 outbox 身份，并与权利行、挑战行的冻结身份摘要互相核对；
//!   任何不符、挑战过期或密文形状错误都失败关闭，直接记 `suppressed`，不发给任何别的名字。
//! - 本模块只读取密文并原样交给调用方，日志与错误不含密文、挑战码或身份文本。

use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{
    ChallengeAad, ClaimedRightNotice, RightNoticeDisposition, RightNoticeFuture, RightNoticeKind,
    RightNoticeLease, RightNoticeSecret, RightNoticeStoreError, RightNoticeWorkStore,
    SealedChallengeSecret,
};
use auction_domain::SettlementWorld;
use auction_time::Timestamp;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::rights::identity_digest;
use crate::database::DatabaseStore;
use crate::database::backend::{
    SERIALIZABLE_ATTEMPTS, is_serialization_conflict, observe_transaction_error, retry_conflicts,
    sql_error_sqlstate,
};
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::recovery::record_notice_receipt;

/// 指纹十六进制文本的字节数，位于 `sealed_payload` 末尾。
const FINGERPRINT_LEN: usize = 64;
/// 单次 `claim_next` 最多顺带抑制的坏工作数，避免长事务链。
const MAX_SUPPRESSED_PER_CLAIM: usize = 32;

/// 事务内失败：可重试的并发冲突，或失败关闭。
enum NoticeTxError {
    Conflict,
    Store(RightNoticeStoreError),
}

impl From<sqlx::Error> for NoticeTxError {
    fn from(error: sqlx::Error) -> Self {
        observe_transaction_error(&error);
        if is_serialization_conflict(&error) {
            return Self::Conflict;
        }
        // 只记录 SQLSTATE 与显式约束名，不读 MESSAGE 或错误链（不变量 7）。
        let constraint = error.as_database_error().and_then(sqlx::error::DatabaseError::constraint);
        tracing::warn!(sqlstate = %sql_error_sqlstate(&error), constraint, "权利通知事务失败");
        Self::Store(RightNoticeStoreError::Unavailable)
    }
}

/// 一次领取尝试的结果。
enum Step {
    Claimed(Box<ClaimedRightNotice>),
    Suppressed,
    Empty,
}

/// 权利通知 outbox 的 PostgreSQL 工作存储。
pub struct PgRightNoticeStore {
    database: DatabaseStore,
    clock: Option<Arc<dyn Fn() -> Timestamp + Send + Sync>>,
}

impl PgRightNoticeStore {
    /// 以共享连接池建立。
    pub fn new(database: DatabaseStore) -> Self {
        Self { database, clock: None }
    }

    /// 测试用：用可控时钟取代数据库墙钟，验证租约过期与围栏。
    #[cfg(feature = "test-db")]
    #[must_use]
    pub fn with_test_clock(mut self, clock: Arc<dyn Fn() -> Timestamp + Send + Sync>) -> Self {
        self.clock = Some(clock);
        self
    }

    async fn now(&self, connection: &mut PgConnection) -> Result<Timestamp, sqlx::Error> {
        match &self.clock {
            Some(clock) => Ok(clock()),
            None => sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(connection).await,
        }
    }

    /// 顶层 READ COMMITTED 事务：工作行 `FOR UPDATE SKIP LOCKED` 或 `FOR UPDATE` 串行化同一工作的竞争。
    async fn run<T, F>(&self, operation: F) -> Result<T, RightNoticeStoreError>
    where
        F: for<'t> Fn(
                &'t Self,
                &'t mut PgConnection,
                Timestamp,
            ) -> futures_util::future::BoxFuture<'t, Result<T, NoticeTxError>>
            + Sync,
    {
        let result = retry_conflicts(
            self.database.pool(),
            self.database.transaction_owner,
            SERIALIZABLE_ATTEMPTS,
            |error| matches!(error, NoticeTxError::Conflict),
            || self.attempt(&operation),
        )
        .await;
        match result {
            Ok(value) => Ok(value),
            Err(NoticeTxError::Store(error)) => Err(error),
            Err(NoticeTxError::Conflict) => Err(RightNoticeStoreError::Unavailable),
        }
    }

    async fn attempt<T, F>(&self, operation: &F) -> Result<T, NoticeTxError>
    where
        F: for<'t> Fn(
                &'t Self,
                &'t mut PgConnection,
                Timestamp,
            ) -> futures_util::future::BoxFuture<'t, Result<T, NoticeTxError>>
            + Sync,
    {
        let mut tx =
            self.database.pool().begin_with("BEGIN ISOLATION LEVEL READ COMMITTED").await?;
        let body = async {
            let now = self.now(&mut tx).await?;
            operation(self, &mut tx, now).await
        }
        .await;
        match body {
            Ok(value) => {
                tx.commit().await?;
                Ok(value)
            }
            Err(error) => {
                if let Err(rollback) = tx.rollback().await {
                    tracing::warn!(error = %rollback, "无法确认权利通知事务已回滚，连接将关闭");
                }
                Err(error)
            }
        }
    }
}

fn world_from_code(code: &str) -> Option<SettlementWorld> {
    match code {
        "main" => Some(SettlementWorld::Main),
        "isekai" => Some(SettlementWorld::Isekai),
        _ => None,
    }
}

fn kind_from_code(code: &str) -> Option<RightNoticeKind> {
    RightNoticeKind::ALL.into_iter().find(|kind| kind.as_str() == code)
}

/// 按布局 `密文 ‖ 64 字节指纹` 拆开 `sealed_payload`。
fn open_payload(
    payload: &[u8],
    nonce: &[u8],
    encryption_key_id: String,
) -> Option<SealedChallengeSecret> {
    let split = payload.len().checked_sub(FINGERPRINT_LEN)?;
    let (ciphertext, fingerprint) = payload.split_at(split);
    let fingerprint = String::from_utf8(fingerprint.to_vec()).ok()?;
    let nonce = <[u8; 12]>::try_from(nonce).ok()?;
    SealedChallengeSecret::new(ciphertext.to_vec(), nonce, encryption_key_id, fingerprint).ok()
}

/// 与 outbox 行对照的挑战事实。
struct ChallengeFacts {
    right_id: Uuid,
    right_version: u64,
    identity_digest: [u8; 32],
    expires_at: Timestamp,
}

async fn challenge_facts(
    connection: &mut PgConnection,
    challenge_id: Uuid,
) -> Result<Option<ChallengeFacts>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT right_id, right_version, identity_digest, expires_at
         FROM public.consignment_right_challenges
         WHERE challenge_id = $1 AND NOT is_virtual",
    )
    .bind(challenge_id)
    .fetch_optional(connection)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let right_id: Option<Uuid> = row.try_get("right_id")?;
    let version: Option<i64> = row.try_get("right_version")?;
    let digest: Option<Vec<u8>> = row.try_get("identity_digest")?;
    let expires_at: Timestamp = row.try_get("expires_at")?;
    // 任一列缺失或形状不合法都按“无可核对的挑战”处理（失败关闭），不替换为零值。
    let (Some(right_id), Some(version), Some(digest)) = (right_id, version, digest) else {
        return Ok(None);
    };
    let (Ok(right_version), Ok(identity_digest)) =
        (u64::try_from(version), <[u8; 32]>::try_from(digest))
    else {
        return Ok(None);
    };
    Ok(Some(ChallengeFacts { right_id, right_version, identity_digest, expires_at }))
}

/// 权利行上的冻结身份与世界，与 outbox 行互相核对。
async fn right_identity(
    connection: &mut PgConnection,
    right_id: Uuid,
) -> Result<Option<(String, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT frozen_identity, world_id FROM public.consignment_rights WHERE right_id = $1",
    )
    .bind(right_id)
    .fetch_optional(connection)
    .await
}

/// 追加结果行，并在同一事务写 `deliver_right_notice` 账本回执与审计（不变量 3）。
async fn insert_result(
    connection: &mut PgConnection,
    work_id: Uuid,
    notice_kind: &str,
    result: &str,
    now: Timestamp,
) -> Result<(), NoticeTxError> {
    sqlx::query(
        "INSERT INTO public.consignment_right_notice_results (work_id, result, occurred_at)
         VALUES ($1, $2, $3)",
    )
    .bind(work_id)
    .bind(result)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    record_notice_receipt(connection, work_id, notice_kind, result).await.map_err(|error| {
        if matches!(error, WriteError::RetryConflict) {
            return NoticeTxError::Conflict;
        }
        tracing::warn!(%error, "权利通知回执写入失败");
        NoticeTxError::Store(RightNoticeStoreError::Unavailable)
    })
}

fn result_code(disposition: RightNoticeDisposition) -> &'static str {
    match disposition {
        RightNoticeDisposition::Sent => "sent",
        RightNoticeDisposition::Unknown => "unknown",
        RightNoticeDisposition::Suppressed => "suppressed",
    }
}

impl PgRightNoticeStore {
    /// 在一个事务里领取至多一条工作；校验不过的工作记 `suppressed` 并报告 `Suppressed`。
    async fn claim_step(
        &self,
        connection: &mut PgConnection,
        now: Timestamp,
        lease: Duration,
    ) -> Result<Step, NoticeTxError> {
        // 只领携带密文的两类；`loss_receipt` / `right_notice` 的正文由 7E 定，这里不领。
        let row = sqlx::query(
            "SELECT work_id, notice_kind, world_id, frozen_identity, challenge_id, right_id,
                    sealed_payload, nonce, encryption_key_id, claim_token, claim_fence
             FROM public.consignment_right_notice_outbox AS outbox
             WHERE notice_kind IN ('locator_guide', 'challenge')
               AND NOT EXISTS (
                   SELECT 1 FROM public.consignment_right_notice_results AS result
                   WHERE result.work_id = outbox.work_id)
               AND (claim_token IS NULL OR lease_expires_at <= $1)
             ORDER BY created_at, work_id
             LIMIT 1
             FOR UPDATE OF outbox SKIP LOCKED",
        )
        .bind(now)
        .fetch_optional(&mut *connection)
        .await?;
        let Some(row) = row else { return Ok(Step::Empty) };
        let work_id: Uuid = row.try_get("work_id")?;
        let kind_code: String = row.try_get("notice_kind")?;
        let world_code: String = row.try_get("world_id")?;
        let frozen_identity: String = row.try_get("frozen_identity")?;
        let challenge_id: Option<Uuid> = row.try_get("challenge_id")?;
        let right_id: Option<Uuid> = row.try_get("right_id")?;
        let payload: Option<Vec<u8>> = row.try_get("sealed_payload")?;
        let nonce: Option<Vec<u8>> = row.try_get("nonce")?;
        let key_id: Option<String> = row.try_get("encryption_key_id")?;
        let previous_token: Option<Uuid> = row.try_get("claim_token")?;
        let previous_fence: i64 = row.try_get("claim_fence")?;

        let verified = self
            .verify_target(
                connection,
                now,
                &world_code,
                &frozen_identity,
                challenge_id,
                right_id,
                (payload, nonce, key_id),
            )
            .await?;
        let (Some(kind), Some(world), Some((aad, sealed))) =
            (kind_from_code(&kind_code), world_from_code(&world_code), verified)
        else {
            insert_result(connection, work_id, &kind_code, "suppressed", now).await?;
            tracing::warn!(kind = "right_notice", "权利通知校验不过，已抑制");
            return Ok(Step::Suppressed);
        };
        let claim_token = Uuid::new_v4();
        let claim_fence = previous_fence + 1;
        let lease_seconds = i64::try_from(lease.as_secs()).unwrap_or(i64::MAX / 4);
        let lease_expires_at = now
            .checked_add_signed(auction_time::Duration::seconds(lease_seconds))
            .ok_or(NoticeTxError::Store(RightNoticeStoreError::Unavailable))?;
        sqlx::query(
            "UPDATE public.consignment_right_notice_outbox
             SET claim_token = $2, claim_fence = $3, lease_expires_at = $4
             WHERE work_id = $1",
        )
        .bind(work_id)
        .bind(claim_token)
        .bind(claim_fence)
        .bind(lease_expires_at)
        .execute(connection)
        .await?;
        Ok(Step::Claimed(Box::new(ClaimedRightNotice {
            lease: RightNoticeLease { work_id, claim_token, claim_fence },
            kind,
            world,
            recipient: frozen_identity,
            secret: RightNoticeSecret { aad, sealed },
            // 令牌仍在说明上一持有者领走后既没完成也没释放：它可能已经发出。
            reclaimed: previous_token.is_some(),
        })))
    }

    /// 核对挑战、权利与 outbox 的冻结身份并拆出密文；任何不符返回 `None`。
    #[allow(clippy::too_many_arguments, reason = "outbox 行的列与校验输入一一对应")]
    async fn verify_target(
        &self,
        connection: &mut PgConnection,
        now: Timestamp,
        world_code: &str,
        frozen_identity: &str,
        challenge_id: Option<Uuid>,
        right_id: Option<Uuid>,
        secret: (Option<Vec<u8>>, Option<Vec<u8>>, Option<String>),
    ) -> Result<Option<(ChallengeAad, SealedChallengeSecret)>, NoticeTxError> {
        let (Some(challenge_id), Some(right_id)) = (challenge_id, right_id) else {
            return Ok(None);
        };
        let (Some(payload), Some(nonce), Some(key_id)) = secret else { return Ok(None) };
        let Some(facts) = challenge_facts(connection, challenge_id).await? else {
            return Ok(None);
        };
        let expected = identity_digest(world_code, frozen_identity);
        let right = right_identity(connection, right_id).await?;
        let identity_matches = facts.right_id == right_id
            && facts.identity_digest == expected
            && right.is_some_and(|(identity, world)| {
                identity == frozen_identity && world == world_code
            });
        if !identity_matches || facts.expires_at <= now {
            return Ok(None);
        }
        let Some(sealed) = open_payload(&payload, &nonce, key_id) else { return Ok(None) };
        let aad = ChallengeAad {
            challenge_id,
            right_id: Some(right_id),
            identity_digest: facts.identity_digest,
            right_version: facts.right_version,
        };
        Ok(Some((aad, sealed)))
    }

    /// 锁住工作行并核对租约；通过则返回通知种类，否则 `LeaseLost`。
    async fn guard_lease(
        connection: &mut PgConnection,
        lease: RightNoticeLease,
    ) -> Result<String, NoticeTxError> {
        let row = sqlx::query(
            "SELECT claim_token, claim_fence, notice_kind,
                    EXISTS (SELECT 1 FROM public.consignment_right_notice_results AS result
                            WHERE result.work_id = outbox.work_id) AS finished
             FROM public.consignment_right_notice_outbox AS outbox
             WHERE work_id = $1
             FOR UPDATE",
        )
        .bind(lease.work_id)
        .fetch_optional(connection)
        .await?;
        let Some(row) = row else {
            return Err(NoticeTxError::Store(RightNoticeStoreError::LeaseLost));
        };
        let token: Option<Uuid> = row.try_get("claim_token")?;
        let fence: i64 = row.try_get("claim_fence")?;
        let finished: bool = row.try_get("finished")?;
        if finished || token != Some(lease.claim_token) || fence != lease.claim_fence {
            return Err(NoticeTxError::Store(RightNoticeStoreError::LeaseLost));
        }
        Ok(row.try_get("notice_kind")?)
    }
}

impl RightNoticeWorkStore for PgRightNoticeStore {
    fn claim_next(&self, lease: Duration) -> RightNoticeFuture<'_, Option<ClaimedRightNotice>> {
        Box::pin(async move {
            for _ in 0..MAX_SUPPRESSED_PER_CLAIM {
                let step = self
                    .run(|store, connection, now| {
                        Box::pin(async move { store.claim_step(connection, now, lease).await })
                    })
                    .await?;
                match step {
                    Step::Claimed(claimed) => return Ok(Some(*claimed)),
                    Step::Empty => return Ok(None),
                    Step::Suppressed => {}
                }
            }
            Ok(None)
        })
    }

    fn complete(
        &self,
        lease: RightNoticeLease,
        disposition: RightNoticeDisposition,
    ) -> RightNoticeFuture<'_, ()> {
        Box::pin(async move {
            self.run(|_, connection, now| {
                Box::pin(async move {
                    let kind = Self::guard_lease(connection, lease).await?;
                    insert_result(connection, lease.work_id, &kind, result_code(disposition), now)
                        .await?;
                    Ok(())
                })
            })
            .await
        })
    }

    fn release(&self, lease: RightNoticeLease) -> RightNoticeFuture<'_, ()> {
        Box::pin(async move {
            self.run(|_, connection, _| {
                Box::pin(async move {
                    Self::guard_lease(connection, lease).await?;
                    sqlx::query(
                        "UPDATE public.consignment_right_notice_outbox
                         SET claim_token = NULL, lease_expires_at = NULL
                         WHERE work_id = $1",
                    )
                    .bind(lease.work_id)
                    .execute(connection)
                    .await?;
                    Ok(())
                })
            })
            .await
        })
    }
}
