//! 内部证据事务 fence；不取得 world、资金或业务行，调用方负责统一锁序。拒绝按类型化原因区分
//! 观察之后凭据或运行时 owner 的正常失效与证据、绑定、时间异常，调用方据此决定延期还是故障。

use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::SafeErrorKind;
use auction_application::settlement::{MailRuntimeBinding, SettlementInternalEvidenceSnapshot};
use auction_domain::SettlementWorld;
use sqlx::{PgConnection, Row};

use crate::database::storage_error;

/// 仅存储适配器内传播的类型化错误；外层重试器先匹配 Sqlx/SQLSTATE 再脱敏出边界，
/// Debug/Display 不输出 PostgreSQL 原文或参数，不能靠错误文本判断重试或分类。
pub enum MailEvidenceTransactionError {
    Sqlx(sqlx::Error),
    Rejected(MailEvidenceRejection),
}

/// 证据 owner 的确定拒绝原因。只有 [`Self::Stale`] 是证据观察之后凭据或运行时 owner 正常推进
/// 造成的失效，属于业务上的确定延期；其余都说明证据、绑定或时钟异常，调用方不得当作正常延期
/// 掩盖，见 [`Self::fault_kind`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailEvidenceRejection {
    /// 凭据已删除、停用（含运行期判定失效）或 `state_version` 已推进，或运行时 generation 已被
    /// 新 owner 推进：观察之后的正常失效，下一轮按当前版本重新准备证据。
    Stale,
    /// 绑定的运行时 generation 非正、高于持久值或持久行缺失：generation 只增不减，这样的绑定
    /// 不可能来自任何已提交的 owner。
    GenerationInvalid,
    /// 证据结构、交叉绑定或摘要校验失败。
    StructureInvalid,
    /// 凭据种类、预期玩家名或作用域与联合证据不符，或持久版本低于证据记录的版本；这些事实在
    /// 凭据创建后不再改写或只增不减，不符只能是绑定错误。
    BindingMismatch,
    /// 按数据库实时时钟证据已过期、含未来观测或观察时刻不在证据时窗内（多为应用与数据库时钟
    /// 偏差）；应用侧预算耗尽在进入事务前已另按超时处理。
    TimeInvalid,
}

impl MailEvidenceRejection {
    /// 固定的运维可见原因，不含证据内容。
    pub const fn message(self) -> &'static str {
        match self {
            Self::Stale => "邮件凭据或运行时 generation 已在观察后失效",
            Self::GenerationInvalid => "邮件运行时 generation 无效",
            Self::StructureInvalid => "邮件证据结构无效",
            Self::BindingMismatch => "邮件凭据与联合证据不匹配",
            Self::TimeInvalid => "邮件证据已过期或包含未来观测",
        }
    }

    /// 低基数原因标签，只用于脱敏日志区分同一安全分类下的不同原因。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Stale => "stale",
            Self::GenerationInvalid => "generation_invalid",
            Self::StructureInvalid => "structure_invalid",
            Self::BindingMismatch => "binding_mismatch",
            Self::TimeInvalid => "time_invalid",
        }
    }

    /// 非正常失效的安全分类；[`Self::Stale`] 返回 `None`，由调用方按业务延期收口。其余原因在
    /// 回滚后必须带这里的分类保留错误信号与计数。
    pub const fn fault_kind(self) -> Option<SafeErrorKind> {
        match self {
            Self::Stale => None,
            Self::GenerationInvalid | Self::StructureInvalid => {
                Some(SafeErrorKind::EVIDENCE_REQUEST_INVALID)
            }
            Self::BindingMismatch => Some(SafeErrorKind::DEPENDENCY_MISMATCH),
            Self::TimeInvalid => Some(SafeErrorKind::EVIDENCE_EXPIRED),
        }
    }
}

impl MailEvidenceTransactionError {
    /// 只允许整事务对 40001/40P01 做有界重试；重试仍复核证据/claim 实时时效。
    pub fn is_serialization_conflict(&self) -> bool {
        matches!(self, Self::Sqlx(error)
            if crate::database::backend::is_serialization_conflict(error))
    }

    /// 仅在存储重试/回滚分类已完成后转换为应用错误，禁止提前丢失 SQLSTATE。
    pub fn into_application_error(self) -> ApplicationError {
        match self {
            Self::Sqlx(error) => crate::database::storage_error_from(error, "邮件证据事务不可用"),
            Self::Rejected(reason) => storage_error(reason.message()),
        }
    }
}

impl From<sqlx::Error> for MailEvidenceTransactionError {
    fn from(error: sqlx::Error) -> Self {
        crate::database::backend::observe_transaction_error(&error);
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
            Self::Rejected(reason) => formatter.write_str(reason.message()),
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
        return Err(MailEvidenceTransactionError::Rejected(
            MailEvidenceRejection::GenerationInvalid,
        ));
    }
    let current = sqlx::query_scalar::<_, i64>(
        "SELECT generation FROM public.mail_runtime_generation
         WHERE scope = 'mail_cookie' FOR UPDATE",
    )
    .fetch_optional(connection)
    .await?;
    match current {
        Some(current) if current == binding.generation => Ok(()),
        // 新 owner 取得所有权时推进 generation：旧绑定在观察之后正常失效。
        Some(current) if current > binding.generation => {
            Err(MailEvidenceTransactionError::Rejected(MailEvidenceRejection::Stale))
        }
        // 持久行缺失或低于绑定：任何已提交的 owner 都不会产生这样的绑定。
        _ => Err(MailEvidenceTransactionError::Rejected(MailEvidenceRejection::GenerationInvalid)),
    }
}

/// 人工资金命令（人工 Sent 证明、装备损失）没有邮件运行时绑定：只在同一锁序首位取 generation
/// 行锁、不校验其值，与持绑定的现金和内部履约事务互斥，不能反向取得 generation/world/account。
pub(crate) async fn lock_mail_runtime_generation_row(
    connection: &mut PgConnection,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "SELECT generation FROM public.mail_runtime_generation WHERE scope='mail_cookie' FOR UPDATE",
    )
    .execute(connection)
    .await?;
    Ok(())
}

/// 按 generation→worlds→funding→auction/recovery/lot→credentials 顺序调用。
/// 仅锁定 active 凭据，预期名称为空时接受真实联合证明；不回填或改写配置名称。
pub async fn lock_mail_evidence_credentials(
    connection: &mut PgConnection,
    snapshot: &SettlementInternalEvidenceSnapshot,
) -> Result<(), MailEvidenceTransactionError> {
    if !snapshot.validate() {
        return Err(MailEvidenceTransactionError::Rejected(
            MailEvidenceRejection::StructureInvalid,
        ));
    }
    let Some(row) = sqlx::query(
        "SELECT credential_kind, status, scope, state_version, player_name
         FROM public.credential_keys WHERE id = $1 FOR UPDATE",
    )
    .bind(snapshot.identity.credential_id)
    .fetch_optional(&mut *connection)
    .await?
    else {
        // Admin 删除凭据后，此前观察到的证据随之失效。
        return Err(MailEvidenceTransactionError::Rejected(MailEvidenceRejection::Stale));
    };
    let kind: String = row.try_get("credential_kind")?;
    let status: String = row.try_get("status")?;
    let scope: String = row.try_get("scope")?;
    let version: i64 = row.try_get("state_version")?;
    let expected: Option<String> = row.try_get("player_name")?;
    let accepts = |world| {
        let world = match world {
            SettlementWorld::Main => auction_application::admin::AdminCredentialWorld::Main,
            SettlementWorld::Isekai => auction_application::admin::AdminCredentialWorld::Isekai,
        };
        scope == world.scope(auction_application::admin::AdminCredentialKind::MailCookie)
            || scope
                == auction_application::admin::AdminCredentialWorld::Shared
                    .scope(auction_application::admin::AdminCredentialKind::MailCookie)
    };
    // 种类、预期玩家名与作用域在凭据创建后不再改写，state_version 只增不减：这些不符先于停用与
    // 版本推进判断，不能被正常失效掩盖。
    if kind != "mail_cookie"
        || version < snapshot.identity.credential_version
        || expected.as_ref().is_some_and(|value| value != &snapshot.identity.player_name)
        || !accepts(snapshot.identity.world)
        || snapshot.equipment.as_ref().is_some_and(|value| !accepts(value.binding.world))
        || snapshot.main_cash.as_ref().is_some_and(|value| !accepts(value.binding.world))
        || snapshot.sent_collection.as_ref().is_some_and(|value| !accepts(value.binding.world))
    {
        return Err(MailEvidenceTransactionError::Rejected(MailEvidenceRejection::BindingMismatch));
    }
    // 停用、运行期判定失效与改名都推进 state_version：观察之后的正常失效。
    if status != "active" || version != snapshot.identity.credential_version {
        return Err(MailEvidenceTransactionError::Rejected(MailEvidenceRejection::Stale));
    }
    validate_mail_evidence_time(connection, snapshot).await
}

/// 最后写入前再次调用；使用真实数据库时间而非事务开始时间。
/// claim lease/操作版本由业务 owner 在同一事务以 clock_timestamp 另行核验。
pub async fn validate_mail_evidence_time(
    connection: &mut PgConnection,
    snapshot: &SettlementInternalEvidenceSnapshot,
) -> Result<(), MailEvidenceTransactionError> {
    // 时效判定内含结构校验；先单独判结构，时间异常与结构异常才不会混成同一原因。
    if !snapshot.validate() {
        return Err(MailEvidenceTransactionError::Rejected(
            MailEvidenceRejection::StructureInvalid,
        ));
    }
    let now = sqlx::query_scalar::<_, auction_time::Timestamp>("SELECT clock_timestamp()")
        .fetch_one(connection)
        .await?;
    if !snapshot.valid_at(now) {
        return Err(MailEvidenceTransactionError::Rejected(MailEvidenceRejection::TimeInvalid));
    }
    Ok(())
}
