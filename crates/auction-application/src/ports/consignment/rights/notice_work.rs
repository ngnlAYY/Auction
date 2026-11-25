//! 权利通知专用 outbox 的工作端口：领取、完成、释放。
//!
//! 不变量 4、5：发送只在本地提交后执行；工作项、租约、围栏与结果全部持久化，内存定时器只用于唤醒。
//! 租约过期后被重新领取的工作（`reclaimed`）无法证明上一次持有者没有发出，必须记 `Unknown` 转人工
//! 对账，绝不自动重发。端口值的 `Debug` 不输出收件身份、密文或挑战码。

use std::fmt;
use std::time::Duration;

use auction_domain::SettlementWorld;
use thiserror::Error;
use uuid::Uuid;

use super::crypto::{ChallengeAad, SealedChallengeSecret};
use super::notice::{RightNoticeDisposition, RightNoticeKind};
use crate::ports::core::TransactionFuture;

/// 工作端口的装箱异步结果。
pub type RightNoticeFuture<'a, T> = TransactionFuture<'a, T, RightNoticeStoreError>;

/// 工作端口对外的安全错误，不保留依赖错误链或输入原文。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum RightNoticeStoreError {
    /// 租约已被他人重新领取、已释放或工作已有结果；持有者必须放弃，不得再写。
    #[error("权利通知租约已失效")]
    LeaseLost,
    /// 存储不可用或事务重试耗尽；失败关闭。
    #[error("权利通知存储暂不可用")]
    Unavailable,
}

/// 一次领取的租约身份：令牌与单调围栏共同证明持有权。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RightNoticeLease {
    /// 工作 ID。
    pub work_id: Uuid,
    /// 本次领取的随机令牌。
    pub claim_token: Uuid,
    /// 本次领取的围栏；每次重新领取加一。
    pub claim_fence: i64,
}

/// 解封所需的密文与 AAD；AAD 取自挑战行，绑定 challenge、权利、冻结身份与权利版本。
pub struct RightNoticeSecret {
    /// 解封用 AAD。
    pub aad: ChallengeAad,
    /// 专用 outbox 里的密文。
    pub sealed: SealedChallengeSecret,
}

impl fmt::Debug for RightNoticeSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("RightNoticeSecret").finish_non_exhaustive()
    }
}

/// 已领取、可以投递的通知。
pub struct ClaimedRightNotice {
    /// 租约。
    pub lease: RightNoticeLease,
    /// 种类（只会是携带挑战码的 `LocatorGuidance` 与 `Challenge`）。
    pub kind: RightNoticeKind,
    /// 收件世界。
    pub world: SettlementWorld,
    /// 收件人：入 outbox 时冻结的游戏身份，不随之后的改名变化。
    pub recipient: String,
    /// 密文与 AAD。
    pub secret: RightNoticeSecret,
    /// 上一次领取没有留下结果也没有释放：不得再发，只能记 `Unknown`。
    pub reclaimed: bool,
}

impl fmt::Debug for ClaimedRightNotice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClaimedRightNotice")
            .field("lease", &self.lease)
            .field("kind", &self.kind)
            .field("world", &self.world)
            .field("reclaimed", &self.reclaimed)
            .finish_non_exhaustive()
    }
}

/// 权利通知专用 outbox 的工作端口。
pub trait RightNoticeWorkStore: Send + Sync {
    /// 领取一条未完成的携带码通知；身份、挑战或过期校验不过的工作在这里直接记 `Suppressed`。
    fn claim_next(&self, lease: Duration) -> RightNoticeFuture<'_, Option<ClaimedRightNotice>>;

    /// 以租约写结果；围栏或令牌不是当前值、或工作已有结果时拒绝 `LeaseLost`。
    fn complete(
        &self,
        lease: RightNoticeLease,
        disposition: RightNoticeDisposition,
    ) -> RightNoticeFuture<'_, ()>;

    /// 明确未发出（临时不可用）时释放租约，让下一轮重新领取；已有结果或租约失效时 `LeaseLost`。
    fn release(&self, lease: RightNoticeLease) -> RightNoticeFuture<'_, ()>;
}
