//! E-Hentai 论坛同步的应用端口与安全快照。
//!
//! 论坛是外部投影，不是拍卖事实来源。Worker 只能领取已持久化的同步任务，读取同一数据库
//! 快照后提交远端证明；远端发帖、回帖解析和竞价命令导入均不得直接修改拍卖状态。

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use auction_domain::{AuctionId, SettlementWorld};

use super::{ApplicationFuture, PublicEquipmentGameLink, PublicEquipmentLinkTarget, RequestHash};

/// 运维侧单次最多列出的论坛未知写入恢复项数。
pub const MAX_FORUM_SYNC_RECOVERY_ITEMS: u16 = 100;

/// 返回固定 #1 的公开布局标记；用于确认首帖属于当前拍卖，而不是只依赖楼层位置。
pub fn forum_catalog_marker(auction_id: AuctionId) -> String {
    format!("auction-catalog-v1:{}", auction_id.as_uuid())
}

/// 返回固定 #2 的公开布局标记；Worker 必须同时核对持久化 post ID，通用解析器不能按正文跳过帖子。
pub fn forum_system_log_marker(auction_id: AuctionId) -> String {
    format!("auction-system-log-v1:{}", auction_id.as_uuid())
}

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
    /// 历史 published 主题的已持久化身份；仅用于首次补齐固定 #1/#2，不允许由标题猜测替代。
    pub existing_thread_id: Option<String>,
    /// 领取前的稳定失败码；过期 `publishing` 租约由存储适配器合成为 `write_unknown`。
    pub previous_failure_code: Option<String>,
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
    /// 不含装备 key 的权威链接目标；完整 URL 只在 Worker 渲染前短暂解析。
    pub equipment_link_target: PublicEquipmentLinkTarget,
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

/// 论坛主题的 BBCode 防腐层；不携带 Cookie、数据库行或外部表单字段。
///
/// 正文可能包含公开装备 URL 的 10 位 key，因此 `Debug` 必须始终脱敏。
#[derive(Clone, PartialEq, Eq)]
pub struct ForumRenderedThread {
    pub title: String,
    pub body: String,
}

/// 固定论坛楼层的 BBCode 正文；内容可能包含公开装备 URL，`Debug` 始终脱敏。
#[derive(Clone, PartialEq, Eq)]
pub struct ForumRenderedPost {
    pub body: String,
}

impl fmt::Debug for ForumRenderedPost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("ForumRenderedPost").field("body", &"[REDACTED]").finish()
    }
}

impl fmt::Debug for ForumRenderedThread {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ForumRenderedThread")
            .field("title", &self.title)
            .field("body", &"[REDACTED]")
            .finish()
    }
}

/// 远端论坛主题的稳定身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumPublishedThread {
    pub board_id: i32,
    pub thread_id: String,
    pub thread_url: String,
}

/// 远端论坛单个帖子的稳定身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumPublishedPost {
    pub thread_id: String,
    pub post_id: String,
}

/// 一场拍卖固定的论坛楼层布局：#1 目录，#2 系统编辑/错误记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumPublishedLayout {
    pub thread: ForumPublishedThread,
    pub catalog_post_id: String,
    pub system_log_post_id: String,
}

/// 论坛未知写入与人工授权状态的安全运维投影。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForumSyncRecoveryItem {
    pub auction_id: Uuid,
    pub status: String,
    pub source_revision: i64,
    pub attempt_count: u32,
    pub failure_code: String,
    pub next_attempt_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 人工确认精确标题对账未命中后，授权 Worker 再次尝试创建主题的幂等命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumWriteRetryAuthorizationCommand {
    pub command_id: Uuid,
    pub auction_id: Uuid,
    pub actor_id: String,
    pub request_hash: RequestHash,
}

/// 人工写入授权结果；实际远端 POST 仍由 Worker 执行，并再次先做精确标题查找。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForumWriteRetryAuthorizationResult {
    pub command_id: Uuid,
    pub auction_id: Uuid,
    pub source_revision: i64,
    pub authorized_at: DateTime<Utc>,
}

/// 论坛同步存储端口。
pub trait ForumSyncStore: Send + Sync {
    /// 领取 due 任务；恢复水位只放行当前 Worker 首轮之前的凭据失败，`None` 表示内存池不可用。
    fn claim_due(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        credential_recovery_before: Option<DateTime<Utc>>,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<ForumSyncClaim>>;

    fn load_snapshot(&self, claim: ForumSyncClaim) -> ApplicationFuture<'_, ForumAuctionSnapshot>;

    fn publish(
        &self,
        claim: ForumSyncClaim,
        rendered: ForumRenderedThread,
        system_log: ForumRenderedPost,
        published: ForumPublishedLayout,
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

/// 当前 Worker 内存凭据池的只读可用性；不暴露 Cookie、scope 或数据库记录。
pub trait ForumCredentialAvailability: Send + Sync {
    fn has_available_forum_credential(&self) -> bool;
}

/// 论坛未知写入的运维恢复端口；SQLite Lite 明确不提供该能力。
pub trait ForumSyncRecoveryPort: Send + Sync {
    fn list_write_recovery(&self, limit: u16) -> ApplicationFuture<'_, Vec<ForumSyncRecoveryItem>>;

    fn authorize_write_retry(
        &self,
        command: ForumWriteRetryAuthorizationCommand,
    ) -> ApplicationFuture<'_, ForumWriteRetryAuthorizationResult>;
}

/// 论坛远程发布/读取端口；实现必须自行处理固定 origin、限速和认证边界。
pub trait ForumPublisher: Send + Sync {
    /// 只读按精确标题查找远端主题；未知写入恢复只能调用该方法，不能隐式创建主题。
    fn find_topic_by_title(
        &self,
        world: SettlementWorld,
        title: String,
    ) -> ApplicationFuture<'_, Option<ForumPublishedThread>>;

    fn create_topic(
        &self,
        world: SettlementWorld,
        rendered: ForumRenderedThread,
    ) -> ApplicationFuture<'_, ForumPublishedThread>;
}

/// 固定论坛楼层写入端口；实现必须校验 exact thread/post 身份并保持 POST 后未知结果栅栏。
pub trait ForumPostWriter: Send + Sync {
    /// 在指定主题追加一条回复；仅用于建立固定 #2。
    fn reply_topic(
        &self,
        world: SettlementWorld,
        thread_id: String,
        body: ForumRenderedPost,
    ) -> ApplicationFuture<'_, ForumPublishedPost>;

    /// 编辑指定稳定 post ID；`title` 仅在编辑 #1 时用于保持主题标题不变。
    fn edit_post(
        &self,
        world: SettlementWorld,
        thread_id: String,
        post_id: String,
        title: Option<String>,
        body: ForumRenderedPost,
    ) -> ApplicationFuture<'_, ForumPublishedPost>;
}

/// 将同一数据库快照转换为不含内部密钥和数据库行的论坛正文。
pub trait ForumRenderer: Send + Sync {
    fn render(
        &self,
        snapshot: ForumAuctionSnapshot,
        equipment_links: Vec<PublicEquipmentGameLink>,
    ) -> Result<ForumRenderedThread, ForumRenderError>;

    /// 渲染固定 #2；水位只表示本轮已经完整处理到的连续楼层。
    fn render_system_log(
        &self,
        auction_id: AuctionId,
        processed_through_post_number: usize,
        entries: Vec<ForumSystemLogEntry>,
    ) -> Result<ForumRenderedPost, ForumRenderError>;
}

/// 固定 #2 中展示的有界处理结果；正文只包含论坛公开帖子与安全错误摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumSystemLogEntry {
    pub post_number: usize,
    /// 论坛返回的真实远端 post ID；跳转链接不得由楼层号推算。
    pub post_id: String,
    pub line_number: Option<usize>,
    pub code: String,
    pub message: String,
    pub ignored: bool,
}

/// 将领域世界转换为旧论坛契约使用的稳定名称。
pub const fn forum_world_name(world: SettlementWorld) -> &'static str {
    match world {
        SettlementWorld::Main => "main",
        SettlementWorld::Isekai => "isekai",
    }
}
