//! 受限权利的 PostgreSQL 实现：权利、挑战、会话、专用 outbox 与找回预算。
//!
//! 端口 [`RightsStore`] 的存储侧。四个浏览器 / 权利人命令（找回、发码、核验、续办）都落在
//! 全局命令账本（actor `browser` / `right_holder`，J5），同 ID 同请求原样重放，异请求冲突。
//! 事务是顶层 READ COMMITTED（理由：竞争发生在“同一命令 / 浏览器 / IP / 身份 / 挑战”这些可枚举的键上，
//! 先取 advisory 锁再读，锁后读到的就是最新已提交事实，不需要快照冲突重试，也不会因序列化失败泄露存在性），临界资源（命令、浏览器、IP、通知身份、挑战、权利）先按固定顺序取
//! 事务级 advisory 锁再读后写，所以“5 次失败”“每身份 10 分钟一封”“预算”在并发下仍精确成立，
//! 不依赖快照冲突重试；40001 / 40P01 只在已确认回滚后以原输入有界重试。
//!
//! 约定（不变量 1、2、3、5、6、7）：
//! - 真实、虚拟与被节流的目标共用 `challenge::issue` 的同一段写账本、写挑战行与写 outbox 代码，
//!   虚拟与被节流只是不写 outbox 行（结构性同形）；429 与目标是否真实无关，且不写任何行。
//! - 挑战码只以 MAC（`RightChallengeCrypto::mac`）入库，投递用密文只进专用 outbox；会话令牌、
//!   浏览器绑定只存 SHA-256 摘要；日志与错误不含任何秘密。
//! - **密钥轮换**：端口 MAC 不带密钥标识。数据密钥轮换后，进行中的挑战无法再通过核验（失败关闭为
//!   `challenge_invalid`），同 ID 的核验重放也失败关闭（重放分支同样先用当前密钥重算 MAC 比对，不符即
//!   `challenge_invalid`，不换发令牌）；持有人需要**重新申请**挑战。
//! - 缺少密码端口时找回、发码、核验失败关闭，不退化为明文保存或跳过 MAC。
//! - 权利行与定位符摘要不可变；本模块只追加挑战、尝试、会话、令牌与 outbox 行。
//!
//! 通知节流与 7D-5 的约定：每个冻结身份（世界 + 身份文本）每 10 分钟至多写一条携带挑战码的
//! outbox 行（`locator_guide` 与 `challenge` 共用同一额度）。找回（locate）发出的码**可核验**：
//! 它是绑定到该身份某一权利的真实挑战（按“该权利最近一次指引最早者优先”轮转），持有人凭它换取
//! 该权利的会话，用于遗失定位符的找回；定位符本身只存摘要，无法重新发放。

mod budget;
mod challenge;
mod claim;
mod entitlement;
mod ledger;
mod notice;
mod outbox;
mod register;
#[allow(
    clippy::module_inception,
    reason = "文件名沿用 R6 §2.2 的 7D-2 清单（rights.rs），7D-5 / 7D-6 按此路径引用"
)]
mod rights;
mod session;

use std::sync::Arc;

use auction_application::consignment::{
    ChallengeIssued, ChallengeRightCommand, ClaimOutcome, ClaimRightCommand, LocateRightCommand,
    RightChallengeCrypto, RightCryptoError, RightReceipt, RightVerified, RightsFuture, RightsStore,
    RightsStoreError, VerifyRightCommand,
};
use auction_domain::CommandId;
use auction_domain::consignment::rights::{RightView, SessionDigest};
use auction_time::Timestamp;
use futures_util::future::BoxFuture;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::backend::{
    SERIALIZABLE_ATTEMPTS, is_serialization_conflict, observe_transaction_error, retry_conflicts,
    set_read_only_limits, sql_error_sqlstate,
};

pub use auction_application::consignment::BrowserBinding;
pub(crate) use claim::{contract_right_ids_in_tx, lock_right_in_tx, refresh_right_state_in_tx};
pub use entitlement::{
    EntitlementDecision, EntitlementRequest, RetainReason, authorize_entitlement_recipient_in_tx,
};
pub use notice::PgRightNoticeStore;
pub(crate) use register::{
    register_contract_rights_in_tx, register_loss_case_right_in_tx, register_position_right_in_tx,
};

/// 命名空间前缀：参与 `hashtextextended`，改名会让新旧进程的锁键错开，**不要改名**。
const LOCK_NAMESPACE: &str = "auction.consignment.rights.v1";

/// 事务内的失败：可重试的并发冲突，或不重试的安全错误。
#[derive(Debug)]
pub(super) enum TxError {
    /// PostgreSQL 明确中止（40001 / 40P01），可以重开整个事务。
    Conflict,
    /// 回滚并以该错误结束，不写任何行。
    Store(RightsStoreError),
}

impl From<RightsStoreError> for TxError {
    fn from(error: RightsStoreError) -> Self {
        Self::Store(error)
    }
}

impl From<RightCryptoError> for TxError {
    fn from(_: RightCryptoError) -> Self {
        // 密码端口错误是无载荷的单元类型：只记错误种类，不记任何密钥或明文；对外一律不可用。
        tracing::warn!(kind = "right_crypto", "受限权利密码端口失败");
        Self::Store(RightsStoreError::Unavailable)
    }
}

impl From<std::num::TryFromIntError> for TxError {
    fn from(_: std::num::TryFromIntError) -> Self {
        Self::Store(RightsStoreError::Unavailable)
    }
}

impl From<sqlx::Error> for TxError {
    fn from(error: sqlx::Error) -> Self {
        observe_transaction_error(&error);
        if is_serialization_conflict(&error) {
            return Self::Conflict;
        }
        // 只记录 SQLSTATE 与显式约束名；不读取 MESSAGE、DETAIL 或错误链（不变量 7）。
        let constraint = error.as_database_error().and_then(sqlx::error::DatabaseError::constraint);
        tracing::warn!(sqlstate = %sql_error_sqlstate(&error), constraint, "受限权利事务失败");
        Self::Store(RightsStoreError::Unavailable)
    }
}

/// 事务内结果：外层 `Err` 回滚；`Ok(Err(_))` 提交（含已落账的拒绝回执）后把拒绝返回给调用方。
pub(super) type Persisted<T> = Result<Result<T, RightsStoreError>, TxError>;

/// 受限权利的 PostgreSQL 存储。
pub struct PgRightsStore {
    database: DatabaseStore,
    crypto: Option<Arc<dyn RightChallengeCrypto>>,
    clock: Option<Arc<dyn Fn() -> Timestamp + Send + Sync>>,
}

impl PgRightsStore {
    /// 以共享连接池与挑战码密码端口建立；密码端口为空时找回、发码、核验失败关闭。
    pub fn new(database: DatabaseStore, crypto: Option<Arc<dyn RightChallengeCrypto>>) -> Self {
        Self { database, crypto, clock: None }
    }

    /// 测试用：用可控时钟取代数据库墙钟，验证 10 分钟窗口与不延寿。
    #[cfg(feature = "test-db")]
    #[must_use]
    pub fn with_test_clock(mut self, clock: Arc<dyn Fn() -> Timestamp + Send + Sync>) -> Self {
        self.clock = Some(clock);
        self
    }

    fn crypto(&self) -> Result<&dyn RightChallengeCrypto, RightsStoreError> {
        self.crypto.as_deref().ok_or(RightsStoreError::Unavailable)
    }

    async fn now(&self, connection: &mut PgConnection) -> Result<Timestamp, sqlx::Error> {
        match &self.clock {
            Some(clock) => Ok(clock()),
            None => sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(connection).await,
        }
    }

    /// 顶层事务：先按文本序取全部 advisory 锁，再读后写；`operation` 只借用事务连接。
    async fn transact<C, T, F>(
        &self,
        lock_keys: Vec<String>,
        input: &C,
        operation: F,
    ) -> Result<T, RightsStoreError>
    where
        C: Sync,
        F: for<'t> Fn(&'t C, &'t mut PgConnection, Timestamp) -> BoxFuture<'t, Persisted<T>> + Sync,
    {
        let mut ordered = lock_keys;
        ordered.sort();
        ordered.dedup();
        let result = retry_conflicts(
            self.database.pool(),
            self.database.transaction_owner,
            SERIALIZABLE_ATTEMPTS,
            |error| matches!(error, TxError::Conflict),
            || self.attempt(&ordered, input, &operation),
        )
        .await;
        match result {
            Ok(outcome) => outcome,
            Err(TxError::Store(error)) => Err(error),
            Err(TxError::Conflict) => Err(RightsStoreError::Unavailable),
        }
    }

    async fn attempt<C, T, F>(
        &self,
        ordered: &[String],
        input: &C,
        operation: &F,
    ) -> Result<Result<T, RightsStoreError>, TxError>
    where
        C: Sync,
        F: for<'t> Fn(&'t C, &'t mut PgConnection, Timestamp) -> BoxFuture<'t, Persisted<T>> + Sync,
    {
        let mut tx =
            self.database.pool().begin_with("BEGIN ISOLATION LEVEL READ COMMITTED").await?;
        let body = async {
            set_read_only_limits(&mut tx)
                .await
                .map_err(|(_setting, error)| TxError::from(error))?;
            for key in ordered {
                sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
                    .bind(key)
                    .execute(&mut *tx)
                    .await?;
            }
            let now = self.now(&mut tx).await?;
            operation(input, &mut tx, now).await
        }
        .await;
        match body {
            Ok(outcome) => {
                // 提交结果未知时调用方以同一 command_id 原样重试，由账本重放回原回执。
                tx.commit().await.map_err(TxError::from)?;
                Ok(outcome)
            }
            Err(error) => {
                if let Err(rollback) = tx.rollback().await {
                    tracing::warn!(error = %rollback, "无法确认受限权利事务已回滚，连接将关闭");
                }
                Err(error)
            }
        }
    }

    /// 只读短事务（会话读取与回执查询）。
    async fn read<C, T, F>(&self, input: &C, operation: F) -> Result<T, RightsStoreError>
    where
        C: Sync,
        F: for<'t> Fn(&'t C, &'t mut PgConnection, Timestamp) -> BoxFuture<'t, Result<T, TxError>>
            + Sync,
    {
        let result: Result<T, TxError> = async {
            let mut tx = self.database.pool().begin_read_snapshot().await?;
            set_read_only_limits(&mut tx)
                .await
                .map_err(|(_setting, error)| TxError::from(error))?;
            let now = self.now(&mut tx).await?;
            let value = operation(input, &mut tx, now).await?;
            tx.commit().await?;
            Ok(value)
        }
        .await;
        match result {
            Ok(value) => Ok(value),
            Err(TxError::Store(error)) => Err(error),
            Err(TxError::Conflict) => Err(RightsStoreError::Unavailable),
        }
    }
}

/// 锁键文本：命名空间、种类与 32 字节以内的原始标识的小写十六进制。
pub(super) fn lock_key(kind: &str, material: &[u8]) -> String {
    let mut key = format!("{LOCK_NAMESPACE}:{kind}:");
    for byte in material {
        key.push_str(&format!("{byte:02x}"));
    }
    key
}

/// 全熵随机字节：由 v4 UUID 的 CSPRNG 取得，丢弃每个 UUID 里被版本与变体占用的两个字节。
pub(super) fn random_bytes<const N: usize>() -> [u8; N] {
    let mut output = [0_u8; N];
    let mut filled = 0;
    while filled < N {
        for (index, byte) in Uuid::new_v4().as_bytes().iter().enumerate() {
            if index == 6 || index == 8 || filled == N {
                continue;
            }
            output[filled] = *byte;
            filled += 1;
        }
    }
    output
}

impl RightsStore for PgRightsStore {
    fn locate(&self, command: LocateRightCommand) -> RightsFuture<'_, ChallengeIssued> {
        Box::pin(async move { self.locate_right(&command).await })
    }

    fn challenge(&self, command: ChallengeRightCommand) -> RightsFuture<'_, ChallengeIssued> {
        Box::pin(async move { self.challenge_right(&command).await })
    }

    fn verify(&self, command: VerifyRightCommand) -> RightsFuture<'_, RightVerified> {
        Box::pin(async move { self.verify_right(&command).await })
    }

    fn current(
        &self,
        session: SessionDigest,
        browser: BrowserBinding,
    ) -> RightsFuture<'_, Option<RightView>> {
        Box::pin(async move { self.current_view(session, browser).await })
    }

    fn claim(&self, command: ClaimRightCommand) -> RightsFuture<'_, ClaimOutcome> {
        Box::pin(async move { self.claim_right(&command).await })
    }

    fn command_receipt(
        &self,
        session: SessionDigest,
        browser: BrowserBinding,
        command_id: CommandId,
    ) -> RightsFuture<'_, Option<RightReceipt>> {
        Box::pin(async move { self.claim_receipt(session, browser, command_id).await })
    }
}
