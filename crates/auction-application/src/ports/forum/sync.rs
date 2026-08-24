//! E-Hentai 论坛同步的应用端口与安全快照。
//!
//! 论坛是外部投影，不是拍卖事实来源。Worker 只能领取已持久化的同步任务，读取同一数据库
//! 快照后提交远端证明；远端发帖、回帖解析和竞价命令导入均不得直接修改拍卖状态。

use chrono::{DateTime, Utc};
use thiserror::Error;
use uuid::Uuid;

use auction_domain::{AuctionId, SettlementWorld};

use super::ApplicationFuture;

/// 论坛远程操作的安全失败分类。
///
/// 只保存稳定错误码和重试栅栏，不保存远端 URL、Cookie、响应正文或提交正文。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
#[error("论坛远程操作失败: code={code} retryable={retryable} write_started={write_started}")]
pub struct ForumRemoteError {
    code: &'static str,
    retryable: bool,
    write_started: bool,
}

/// 论坛本地主题渲染失败分类。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ForumRenderError {
    #[error("论坛拍卖快照无效")]
    InvalidSnapshot,
    #[error("论坛拍卖主题正文超出远端限制")]
    BodyTooLarge,
}

impl ForumRemoteError {
    /// 建立一个持久化安全的远程错误。
    pub const fn new(code: &'static str, retryable: bool, write_started: bool) -> Self {
        Self { code, retryable, write_started }
    }

    /// 返回稳定错误码。
    pub const fn code(self) -> &'static str {
        self.code
    }

    /// 返回是否允许 Worker 退避重试。
    pub const fn retryable(self) -> bool {
        self.retryable
    }

    /// 返回是否已经进入远端写入阶段。
    pub const fn write_started(self) -> bool {
        self.write_started
    }
}

/// 论坛同步任务的安全 claim。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumSyncClaim {
    pub auction_id: AuctionId,
    pub claim_token: Uuid,
    pub claim_fencing: i64,
    pub worker_id: String,
    pub claimed_at: DateTime<Utc>,
    pub lease_until: DateTime<Utc>,
    pub source_revision: i64,
}

/// 论坛主题中展示的单个 lot 快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumLotSnapshot {
    pub code: String,
    pub category: String,
    pub name: String,
    pub seller: String,
    pub start_price: i64,
    pub current_price: i64,
    pub status: String,
    pub current_bidder: Option<String>,
    pub effective_end_at: Option<DateTime<Utc>>,
}

/// 论坛主题渲染所需的同源拍卖快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumAuctionSnapshot {
    pub auction_id: AuctionId,
    pub world: SettlementWorld,
    pub sequence: i64,
    pub state_version: i64,
    pub scheduled_open_at: DateTime<Utc>,
    pub opened_at: Option<DateTime<Utc>>,
    pub scheduled_end_at: Option<DateTime<Utc>>,
    pub public_url: Option<String>,
    pub lots: Vec<ForumLotSnapshot>,
}

/// 论坛主题的纯文本防腐层；不携带 Cookie、数据库行或外部表单字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumRenderedThread {
    pub title: String,
    pub body: String,
}

/// 远端论坛主题的稳定身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumPublishedThread {
    pub board_id: i32,
    pub thread_id: String,
    pub thread_url: String,
}

/// 论坛同步存储端口。
pub trait ForumSyncStore: Send + Sync {
    fn claim_due(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<ForumSyncClaim>>;

    fn load_snapshot(&self, claim: ForumSyncClaim) -> ApplicationFuture<'_, ForumAuctionSnapshot>;

    fn publish(
        &self,
        claim: ForumSyncClaim,
        rendered: ForumRenderedThread,
        published: ForumPublishedThread,
        published_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()>;

    fn fail(
        &self,
        claim: ForumSyncClaim,
        retry_at: DateTime<Utc>,
        failure_code: String,
        safe_detail: String,
    ) -> ApplicationFuture<'_, ()>;
}

/// 论坛远程发布/读取端口；实现必须自行处理固定 origin、限速和认证边界。
pub trait ForumPublisher: Send + Sync {
    fn create_topic(
        &self,
        world: SettlementWorld,
        rendered: ForumRenderedThread,
    ) -> ApplicationFuture<'_, ForumPublishedThread>;
}

/// 将同一数据库快照转换为不含内部密钥和数据库行的论坛正文。
pub trait ForumRenderer: Send + Sync {
    fn render(
        &self,
        snapshot: ForumAuctionSnapshot,
    ) -> Result<ForumRenderedThread, ForumRenderError>;
}

/// 将领域世界转换为旧论坛契约使用的稳定名称。
pub const fn forum_world_name(world: SettlementWorld) -> &'static str {
    match world {
        SettlementWorld::Main => "main",
        SettlementWorld::Isekai => "isekai",
    }
}
