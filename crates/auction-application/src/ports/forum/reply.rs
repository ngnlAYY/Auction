//! 论坛回帖扫描与竞价命令导入端口。
//!
//! 论坛帖子只是外部输入。Worker 必须先通过身份和 lot 投影解析，再使用现有竞价命令端口；
//! 本模块不允许论坛适配器直接写拍卖状态。

use chrono::{DateTime, Utc};
use uuid::Uuid;

use auction_domain::{AuctionId, LotId, ProxyAuthorizationId, SettlementWorld, UserId};

use super::{AdminForumPreviewResult, ApplicationFuture};

/// 已发布论坛主题的最小扫描目标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumReplyTopic {
    pub auction_id: AuctionId,
    pub world: SettlementWorld,
    pub thread_id: String,
}

/// 论坛目录编码到数据库 lot 的安全映射。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumLotTarget {
    pub code: String,
    pub lot_id: LotId,
}

/// 已登记且仍可参与竞价的论坛身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumBidderIdentity {
    pub user_id: UserId,
    pub external_account_id: String,
    pub display_name: String,
}

/// 用户在指定 lot 上的当前活动代理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForumActiveProxy {
    pub authorization_id: ProxyAuthorizationId,
}

/// 论坛命令账本的幂等事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumBidCommandRecord {
    pub command_id: Uuid,
    pub auction_id: AuctionId,
    pub world: SettlementWorld,
    pub thread_id: String,
    pub post_id: String,
    pub line_number: i32,
    pub lot_code: String,
    pub actor: String,
    pub command_kind: String,
    pub amount: i64,
}

/// 论坛命令账本当前状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForumCommandStatus {
    Pending,
    Applied,
    Rejected,
}

/// 论坛命令账本开始结果。
///
/// Worker 需要区分“已完成的原命令”和“本次可以继续执行的 pending 命令”，否则竞价已成功
/// 但通知入队失败时，下一轮只看到 `should_run = false`，会永久跳过通知重试。旧版账本使用
/// 不同的稳定 ID 算法，因此还必须返回实际命中的命令 ID，让后续竞价、通知和完成写入继续
/// 使用同一个幂等身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForumCommandStart {
    pub command_id: Uuid,
    pub status: ForumCommandStatus,
    pub should_run: bool,
}

/// 论坛回帖持久化端口。
pub trait ForumReplyStore: Send + Sync {
    fn list_published_topics(&self, limit: u16) -> ApplicationFuture<'_, Vec<ForumReplyTopic>>;

    fn lot_targets(&self, auction_id: AuctionId) -> ApplicationFuture<'_, Vec<ForumLotTarget>>;

    fn resolve_bidder(
        &self,
        world: SettlementWorld,
        external_account_id: String,
        display_name: String,
    ) -> ApplicationFuture<'_, ForumBidderIdentity>;

    fn active_proxy(
        &self,
        lot_id: LotId,
        user_id: UserId,
    ) -> ApplicationFuture<'_, Option<ForumActiveProxy>>;

    fn begin_command(
        &self,
        record: ForumBidCommandRecord,
    ) -> ApplicationFuture<'_, ForumCommandStart>;

    fn finish_command(
        &self,
        command_id: Uuid,
        status: String,
        error_code: String,
        safe_detail: String,
    ) -> ApplicationFuture<'_, ()>;

    fn mark_scanned(
        &self,
        world: SettlementWorld,
        scanned_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()>;
}

/// 论坛主题读取端口；实现必须在外部适配器内完成固定 origin、认证和响应边界。
pub trait ForumTopicReader: Send + Sync {
    fn fetch_topic(
        &self,
        world: SettlementWorld,
        thread_id: String,
    ) -> ApplicationFuture<'_, AdminForumPreviewResult>;
}
