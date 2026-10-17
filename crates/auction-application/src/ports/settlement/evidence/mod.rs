//! 联合远端证据与同会话生命周期；证据只证明观察事实，不授予资金或履约权限。

use std::{future::Future, pin::Pin, time::Instant};

mod context;
mod model;
mod sent;
pub use context::*;
pub use model::*;
pub use sent::*;

/// 全局邮件运行时锁域；不能按凭据版本或世界拆分。
pub const MAIL_RUNTIME_SCOPE: &str = "mail_cookie";
/// 仅覆盖已批准的联合身份、organize 正向库存与干净 compose 协议。
pub const INTERNAL_EVIDENCE_PROTOCOL: &str = "hv_uid_forum_profile_armory_v1";

/// 只读失败均不产生外部 write_unknown，也不能被解释成“不是 bot”。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InternalEvidenceError {
    #[error("当前凭据或适配器不支持内部证据")]
    Unsupported,
    #[error("远端事实未获证明")]
    Unproven,
    #[error("证据依赖暂不可用")]
    Unavailable,
    #[error("证据预算或有效期已过")]
    Expired,
    #[error("证据运行时所有权已撤销")]
    Revoked,
    #[error("证据请求无效")]
    InvalidRequest,
}

/// 绝对预算内的只读证据 future。
pub type InternalEvidenceFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, InternalEvidenceError>> + Send + 'a>>;

/// 在一个独占 session 内准备身份、库存及可选现金观察。
pub trait SettlementInternalEvidenceReader: Send + Sync {
    fn prepare_internal_evidence(
        &self,
        request: InternalEvidenceRequest,
    ) -> InternalEvidenceFuture<'_, PreparedSettlementInternalEvidence>;

    /// 在同一 guard/预算内追加精确 Sent 领取观察；旧适配器明确不支持。
    fn prepare_internal_evidence_with_sent(
        &self,
        _request: InternalEvidenceRequest,
        _target: InternalSentCollectionTarget,
    ) -> InternalEvidenceFuture<'_, PreparedSettlementInternalEvidence> {
        Box::pin(async { Err(InternalEvidenceError::Unsupported) })
    }
}

/// 外部 UID/EID 使用 canonical 正十进制文本，范围与既有 bigint 标识一致。
pub fn valid_external_positive_id(value: &str) -> bool {
    !value.starts_with('0')
        && !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.parse::<i64>().is_ok_and(|number| number > 0)
}

/// 固定预算上限；排队、限速、所有 GET/body 以及提交共享此截止点。
pub fn internal_evidence_deadline(requested: Instant) -> Instant {
    requested.min(Instant::now() + std::time::Duration::from_secs(30))
}
