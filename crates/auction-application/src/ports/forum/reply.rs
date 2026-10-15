//! 论坛回帖扫描与竞价命令导入端口。
//!
//! 论坛帖子只是外部输入。Worker 必须先通过身份和 lot 投影解析，再使用现有竞价命令端口；
//! 本模块不允许论坛适配器直接写拍卖状态。

use std::fmt;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use auction_domain::{AuctionId, LotId, ProxyAuthorizationId, SettlementWorld, UserId};

use super::{
    AdminForumPreviewResult, ApplicationFuture, ForumAuctionSnapshot, ForumRenderedPost,
    ForumRenderedThread,
};

/// 已发布论坛主题的最小扫描目标。
#[derive(Clone, PartialEq, Eq)]
pub struct ForumReplyTopic {
    pub auction_id: AuctionId,
    pub world: SettlementWorld,
    pub thread_id: String,
    pub catalog_post_id: String,
    pub system_log_post_id: String,
    pub title: String,
    pub catalog_body: String,
    pub system_log_body: String,
    pub content_revision: i64,
    pub processed_through_post_number: i32,
    pub content_claim_token: Uuid,
    pub content_claim_owner: String,
    pub content_claim_fencing: i64,
    pub content_lease_until: DateTime<Utc>,
    /// 上一轮未恢复的安全错误码；`write_unknown` 只能先只读对账，正文是否逐字核对由运行策略决定。
    pub content_last_error_code: Option<String>,
}

impl fmt::Debug for ForumReplyTopic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ForumReplyTopic")
            .field("auction_id", &self.auction_id)
            .field("world", &self.world)
            .field("thread_id", &self.thread_id)
            .field("catalog_post_id", &self.catalog_post_id)
            .field("system_log_post_id", &self.system_log_post_id)
            .field("title", &self.title)
            .field("catalog_body", &"[REDACTED]")
            .field("system_log_body", &"[REDACTED]")
            .field("content_revision", &self.content_revision)
            .field("processed_through_post_number", &self.processed_through_post_number)
            .field("content_claim_token", &"[REDACTED]")
            .field("content_claim_owner", &self.content_claim_owner)
            .field("content_claim_fencing", &self.content_claim_fencing)
            .field("content_lease_until", &self.content_lease_until)
            .field("content_last_error_code", &self.content_last_error_code)
            .finish()
    }
}

/// 已持久化论坛命令的安全结果，用于固定 #2 的处理摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumCommandOutcome {
    pub post_id: String,
    /// 命令首次由完整主题读取证明的真实楼层；历史不可证明时保持空。
    pub post_number: Option<i32>,
    pub line_number: i32,
    pub lot_code: String,
    pub command_kind: String,
    pub status: ForumCommandStatus,
    /// 持久化拒绝分类；Worker 仅对白名单错误码生成论坛英文提示。
    pub error_code: Option<String>,
    /// 本地有界诊断；不得原样发送到论坛，仅允许已知历史格式的只读映射。
    pub safe_detail: Option<String>,
}

/// 论坛目录编码到数据库 lot 的安全映射。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumLotTarget {
    pub code: String,
    pub lot_id: LotId,
}

/// 由论坛账号数字键与论坛/MM 共用玩家名称解析且仍可参与竞价的用户身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumBidderIdentity {
    pub user_id: UserId,
    /// 论坛 `showuser` 的稳定数字键，只用于来源映射和命令账本 actor。
    pub external_account_id: String,
    /// 当前论坛名称，也是 MoogleMail 使用的可变收件人名称。
    pub player_name: String,
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
    /// 完整主题读取时观察到的真实楼层号；不得由 post ID 推算。
    pub post_number: i32,
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
    /// 领取固定楼层刷新目标；内容 lease 必须覆盖单主题总时限，避免多 Worker 交叉编辑远端帖子。
    fn claim_published_topics(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<ForumReplyTopic>>;

    fn lot_targets(&self, auction_id: AuctionId) -> ApplicationFuture<'_, Vec<ForumLotTarget>>;

    fn resolve_bidder(
        &self,
        world: SettlementWorld,
        external_account_id: String,
        player_name: String,
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

    /// 读取命令账本的确定结果，不返回凭据或请求正文。
    fn command_outcomes(
        &self,
        auction_id: AuctionId,
        thread_id: String,
    ) -> ApplicationFuture<'_, Vec<ForumCommandOutcome>>;

    /// 读取当前权威拍卖快照，用于竞价命令提交后刷新固定 #1。
    fn load_current_snapshot(
        &self,
        auction_id: AuctionId,
    ) -> ApplicationFuture<'_, ForumAuctionSnapshot>;

    /// 在两个远端固定楼层均已得到 exact post proof 后，以 revision CAS 更新本地正文证明并推进下次扫描 due。
    fn record_fixed_posts_update(
        &self,
        topic: ForumReplyTopic,
        catalog: ForumRenderedThread,
        system_log: ForumRenderedPost,
        processed_through_post_number: i32,
        updated_at: DateTime<Utc>,
        next_scan_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()>;

    /// 暂时失败时只记录已取得 exact proof 的固定楼层正文，保留当前 claim 与到期后的重试水位。
    fn record_fixed_posts_progress(
        &self,
        topic: ForumReplyTopic,
        catalog: ForumRenderedThread,
        system_log: ForumRenderedPost,
        processed_through_post_number: i32,
        updated_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()>;

    /// 本轮没有正文变化时释放内容 lease，并把公平扫描水位推进到调用方给出的下一周期。
    fn finish_topic_scan(
        &self,
        topic: ForumReplyTopic,
        processed_through_post_number: i32,
        scanned_at: DateTime<Utc>,
        next_scan_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()>;

    /// 失败时释放当前 claim、记录有界错误码并推进单主题重试水位。
    fn fail_topic_scan(
        &self,
        topic: ForumReplyTopic,
        failed_at: DateTime<Utc>,
        retry_at: DateTime<Utc>,
        failure_code: String,
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
