//! 内部证据事务 fence；不取得 world、资金或业务行，调用方负责统一锁序。

use auction_application::{
    ApplicationError, MailRuntimeBinding, SettlementInternalEvidenceSnapshot,
};
use auction_domain::SettlementWorld;
use sqlx::{PgConnection, Row};

use super::common::storage_error;

/// 仅存储适配器内传播的类型化错误；外层重试器先匹配 Sqlx/SQLSTATE 再脱敏出边界，
/// Debug/Display 不输出 PostgreSQL 原文或参数，不能靠错误文本判断重试。
pub enum MailEvidenceTransactionError {
    Sqlx(sqlx::Error),
    Rejected(&'static str),
}

impl MailEvidenceTransactionError {
    /// 只允许整事务对 40001/40P01 做有界重试；重试仍复核证据/claim 实时时效。
    pub fn is_serialization_conflict(&self) -> bool {
        matches!(self, Self::Sqlx(error) if crate::database::is_serialization_conflict(error))
    }

    /// 仅在存储重试/回滚分类已完成后转换为应用错误，禁止提前丢失 SQLSTATE。
    pub fn into_application_error(self) -> ApplicationError {
        match self {
            Self::Sqlx(error) => crate::database::storage_error_from(error, "邮件证据事务不可用"),
            Self::Rejected(reason) => storage_error(reason),
        }
    }
}

impl From<sqlx::Error> for MailEvidenceTransactionError {
    fn from(error: sqlx::Error) -> Self {
        Self::Sqlx(error)
    }
}

impl std::fmt::Debug for MailEvidenceTransactionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, formatter)
    }
}

impl std::fmt::Display for MailEvidenceTransactionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlx(_) => formatter.write_str("邮件证据事务数据库错误"),
            Self::Rejected(reason) => formatter.write_str(reason),
        }
    }
}

impl std::error::Error for MailEvidenceTransactionError {}

/// 必须在业务事务的第一把行锁位置调用并持锁到确定提交/回滚；
/// 旧 generation 永远失败，不因当前凭据或 UID 相同而复活。
pub async fn lock_mail_runtime_generation(
    connection: &mut PgConnection,
    binding: &MailRuntimeBinding,
) -> Result<(), MailEvidenceTransactionError> {
    if binding.generation <= 0 {
        return Err(MailEvidenceTransactionError::Rejected("邮件运行时 generation 无效"));
    }
    let current = sqlx::query_scalar::<_, i64>(
        "SELECT generation FROM public.mail_runtime_generation
         WHERE scope = 'mail_cookie' FOR UPDATE",
    )
    .fetch_optional(connection)
    .await?;
    if current != Some(binding.generation) {
        return Err(MailEvidenceTransactionError::Rejected("邮件运行时 generation 已失效"));
    }
    Ok(())
}

/// 按 generation→worlds→funding→auction/recovery/lot→credentials 顺序调用。
/// 仅锁定 active 凭据，预期名称为空时接受真实联合证明；不回填或改写配置名称。
pub async fn lock_mail_evidence_credentials(
    connection: &mut PgConnection,
    snapshot: &SettlementInternalEvidenceSnapshot,
) -> Result<(), MailEvidenceTransactionError> {
    if !snapshot.validate() {
        return Err(MailEvidenceTransactionError::Rejected("邮件证据结构无效"));
    }
    let row = sqlx::query(
        "SELECT credential_kind, status, scope, state_version, player_name
         FROM public.credential_keys WHERE id = $1 FOR UPDATE",
    )
    .bind(snapshot.identity.credential_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(MailEvidenceTransactionError::Rejected("邮件凭据已不存在"))?;
    let kind: String = row.try_get("credential_kind")?;
    let status: String = row.try_get("status")?;
    let scope: String = row.try_get("scope")?;
    let version: i64 = row.try_get("state_version")?;
    let expected: Option<String> = row.try_get("player_name")?;
    let accepts = |world| {
        let world = match world {
            SettlementWorld::Main => auction_application::AdminCredentialWorld::Main,
            SettlementWorld::Isekai => auction_application::AdminCredentialWorld::Isekai,
        };
        scope == world.scope(auction_application::AdminCredentialKind::MailCookie)
            || scope
                == auction_application::AdminCredentialWorld::Shared
                    .scope(auction_application::AdminCredentialKind::MailCookie)
    };
    if kind != "mail_cookie"
        || status != "active"
        || version != snapshot.identity.credential_version
        || expected.as_ref().is_some_and(|value| value != &snapshot.identity.player_name)
        || !accepts(snapshot.identity.world)
        || snapshot.equipment.as_ref().is_some_and(|value| !accepts(value.binding.world))
        || snapshot.main_cash.as_ref().is_some_and(|value| !accepts(value.binding.world))
        || snapshot.sent_collection.as_ref().is_some_and(|value| !accepts(value.binding.world))
    {
        return Err(MailEvidenceTransactionError::Rejected("邮件凭据与联合证据不匹配"));
    }
    validate_mail_evidence_time(connection, snapshot).await
}

/// 最后写入前再次调用；使用真实数据库时间而非事务开始时间。
/// claim lease/操作版本由业务 owner 在同一事务以 clock_timestamp 另行核验。
pub async fn validate_mail_evidence_time(
    connection: &mut PgConnection,
    snapshot: &SettlementInternalEvidenceSnapshot,
) -> Result<(), MailEvidenceTransactionError> {
    let now = sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>("SELECT clock_timestamp()")
        .fetch_one(connection)
        .await?;
    if !snapshot.valid_at(now) {
        return Err(MailEvidenceTransactionError::Rejected("邮件证据已过期或包含未来观测"));
    }
    Ok(())
}
