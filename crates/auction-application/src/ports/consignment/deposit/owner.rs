//! 隔离重检时判定一封系统退件是否属于既有拍卖 owner 的窄端口。

use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::deposit::{EnvelopeObservation, LegacyAuctionOwner};

use crate::ports::consignment::ConsignmentFuture;

/// 只读判定旧拍卖 owner；查不到或证据不足返回 `None`，由调用方保持隔离。
///
/// 实现必须只使用系统主题、正文后缀与本地义务/恢复记录，不得把“看起来像”当作归属。
/// 返回 `Some` 只表示可以按旧 owner 的既有判据继续，不授予任何资产动作。
pub trait LegacyAuctionOwnerResolver: Send + Sync {
    /// 按原信键与本次观察判定；未知归属失败关闭为 `None`。
    fn resolve(
        &self,
        key: &OriginalMailKey,
        observation: &EnvelopeObservation,
    ) -> ConsignmentFuture<'_, Option<LegacyAuctionOwner>>;
}
