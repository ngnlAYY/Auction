//! E-Hentai 论坛同步的应用端口与安全快照。
//!
//! 论坛是外部投影，不是拍卖事实来源。Worker 只能领取已持久化的同步任务，读取同一数据库
//! 快照后提交远端证明；远端发帖、回帖解析和竞价命令导入均不得直接修改拍卖状态。

use std::fmt;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use auction_domain::{AuctionId, SettlementWorld};

use super::{ApplicationFuture, PublicEquipmentGameLink, PublicEquipmentLinkTarget, RequestHash};

/// 运维侧单次最多列出的论坛未知写入恢复项数。
pub const MAX_FORUM_SYNC_RECOVERY_ITEMS: u16 = 100;

/// 每个目录帖子最多展示的装备件数。
pub const FORUM_CATALOG_PAGE_SIZE: usize = 150;

/// 论坛快照的拍品总数边界；分页不能绕过总量限制。
pub const MAX_FORUM_CATALOG_LOTS: usize = 10_000;

/// 有界目录页数；与总量及每帖件数保持一致。
pub const MAX_FORUM_CATALOG_PAGES: usize = MAX_FORUM_CATALOG_LOTS.div_ceil(FORUM_CATALOG_PAGE_SIZE);

/// 返回固定 #1 的公开布局标记；用于确认首帖属于当前拍卖，而不是只依赖楼层位置。
pub fn forum_catalog_marker(auction_id: AuctionId) -> String {
    format!("auction-catalog-v1:{}", auction_id.as_uuid())
}

/// 返回目录页的公开标记；首帖沿用历史标记，续页绑定拍卖与真实楼层。
pub fn forum_catalog_page_marker(auction_id: AuctionId, post_number: usize) -> String {
    if post_number == 1 {
        forum_catalog_marker(auction_id)
    } else {
        format!("auction-catalog-page-v1:{}:{post_number}", auction_id.as_uuid())
    }
}

/// 返回系统日志的公开布局标记；Worker 必须同时核对持久化 post ID，通用解析器不能按正文跳过帖子。
pub fn forum_system_log_marker(auction_id: AuctionId) -> String {
    format!("auction-system-log-v1:{}", auction_id.as_uuid())
}

/// 比较固定楼层的原始 BBCode，仅兼容论坛保存时去掉正文末尾 CR/LF 的行为。
///
/// 正文内部的换行、空格及 BBCode 仍须逐字相等，不能用宽泛的空白折叠掩盖内容差异。
pub fn forum_post_bodies_match(observed_body: &str, expected_body: &str) -> bool {
    observed_body.trim_end_matches(['\r', '\n']) == expected_body.trim_end_matches(['\r', '\n'])
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

/// 单次论坛内容扫描的远端写入进度；仅记录是否存在尚未确认的 POST。
///
/// 传输适配器在预读、限速和凭据校验全部完成、即将发送 POST 时标记。调用方必须取得精确帖子回执；
/// 启用正文校验时还须经 [`forum_post_bodies_match`] 确认原始正文。该对象不替代持久化的未知结果门禁。
#[derive(Debug, Clone, Default)]
pub struct ForumWriteProgress {
    unconfirmed: Arc<AtomicBool>,
}

impl ForumWriteProgress {
    /// 在实际 POST 发送前标记，供外层超时取消后识别不确定的外部写入。
    pub fn mark_write_started(&self) {
        self.unconfirmed.store(true, Ordering::Release);
    }

    /// 已取得精确帖子回执及配置要求的正文证明，解除本轮这一笔写入的未知标记。
    pub fn confirm_write(&self) {
        self.unconfirmed.store(false, Ordering::Release);
    }

    /// 返回本轮是否已经发送过尚未通过所选确认策略的 POST。
    pub fn has_unconfirmed_write(&self) -> bool {
        self.unconfirmed.load(Ordering::Acquire)
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
    /// 历史 published 主题的已持久化身份；仅用于首次补齐固定楼层，不允许由标题猜测替代。
    pub existing_thread_id: Option<String>,
    /// 领取前的稳定失败码；过期 `publishing` 租约由存储适配器合成为 `write_unknown`。
    pub previous_failure_code: Option<String>,
}

/// 论坛目录中当前领先事实的可证明来源。
///
/// 论坛来源只携带真实主题、帖子和楼层身份；代理只保留统一标记，不暴露 fast/slow 模式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForumCurrentBidSource {
    Forum { post_number: i32, thread_id: String, post_id: String },
    Proxy,
}

/// 已结算成交拍品的只读展示事实；调整来自持久结算，不含补偿池额外抵扣。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumLotSettlement {
    pub leader_since: DateTime<Utc>,
    pub adjustment_ppm: i64,
}

/// 论坛主题中展示的单个 lot 快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumLotSnapshot {
    pub code: String,
    pub category: String,
    pub name: String,
    pub equipment_level: Option<i64>,
    /// 按 HentaVerse Primary Attributes 固定顺序生成的大写首字母缩写。
    pub primary_attribute_initials: Option<String>,
    /// 平均 Roll 百分比乘以十后的整数，避免论坛快照引入浮点相等语义。
    pub avg_roll_tenths: Option<i64>,
    pub seller: String,
    pub start_price: i64,
    pub current_price: i64,
    pub status: String,
    pub current_bidder: Option<String>,
    pub current_bid_source: Option<ForumCurrentBidSource>,
    pub effective_end_at: Option<DateTime<Utc>>,
    pub settlement: Option<ForumLotSettlement>,
    /// 不含装备 key 的权威链接目标；完整 URL 只在 Worker 渲染前短暂解析。
    pub equipment_link_target: PublicEquipmentLinkTarget,
}

/// 首次发布前冻结的论坛目录布局；旧主题不因升级挪动日志或既有用户楼层。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForumCatalogLayout {
    /// 已开始发布的历史主题保持单页，仍执行原正文长度上限。
    LegacySingle,
    /// 新主题从 #1 起每帖最多 150 件，更新楼在最后一页之后。
    PagedV1,
}

impl ForumCatalogLayout {
    /// 返回冻结布局的分帖边界；不能从当前装备数量猜测旧帖布局。
    pub const fn page_size(self) -> usize {
        match self {
            Self::LegacySingle => MAX_FORUM_CATALOG_LOTS,
            Self::PagedV1 => FORUM_CATALOG_PAGE_SIZE,
        }
    }
}

/// 论坛主题渲染所需的同源拍卖快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumAuctionSnapshot {
    pub auction_id: AuctionId,
    pub catalog_layout: ForumCatalogLayout,
    pub world: SettlementWorld,
    pub sequence: i64,
    pub state_version: i64,
    /// 仅权威 settled 状态且全部结算明细齐备时为 true，不由墙钟推断。
    pub settled: bool,
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
    /// 远端主题标题；必须包含拍卖世界，且参与主题精确对账。
    pub title: String,
    /// 远端 Topic Description；只放时间等主题备注，不混入目录正文。
    pub description: String,
    /// 旧版标题兼容值；仅用于标题迁移期间的精确只读对账。
    pub legacy_title: Option<String>,
    pub body: String,
    /// 从 #2 起依次发布的目录续页；系统日志必须位于这些页面之后。
    pub catalog_continuation: Vec<ForumRenderedPost>,
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
            .field("description", &self.description)
            .field("legacy_title", &self.legacy_title)
            .field("body", &"[REDACTED]")
            .field("catalog_continuation", &self.catalog_continuation)
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

/// 创建请求的结果来源；找到同标题旧主题不等于已发送本轮目录正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForumTopicPublication {
    /// 实际 POST 创建成功且远端主题回执匹配。
    Created(ForumPublishedThread),
    /// 只读精确标题命中；固定帖内容仍需证明或人工授权更新。
    Existing(ForumPublishedThread),
}

/// 编辑论坛主题元数据；`None` 字段保持服务器现值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumTopicMetadata {
    pub title: Option<String>,
    pub description: Option<String>,
}

/// 远端论坛单个帖子的稳定身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumPublishedPost {
    pub thread_id: String,
    pub post_id: String,
}

/// 一场拍卖固定的论坛楼层布局：连续目录页之后紧接系统编辑/错误记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumPublishedLayout {
    pub thread: ForumPublishedThread,
    pub catalog_post_id: String,
    /// 按真实楼层顺序保存 #2 起的目录续页身份，不由 post ID 推算楼层。
    pub catalog_continuation_post_ids: Vec<String>,
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
    /// 领取 due 任务；启动恢复水位只放行首轮前的凭据失败及未知写入只读对账，`None` 表示凭据不可用。
    /// 恢复必须将原失败码传入 claim，不授予 POST 权限；待人工写入授权的任务仍不领取。
    fn claim_due(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        startup_recovery_before: Option<DateTime<Utc>>,
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

/// 当前 Worker 单 Cookie 的只读可用性；不暴露 Cookie、scope 或数据库记录。
pub trait ForumCredentialAvailability: Send + Sync {
    fn has_available_forum_credential(&self) -> bool;
}

/// 论坛未知写入的运维恢复端口。
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

    /// 创建或找到既有主题，必须明确本轮是否实际完成创建 POST。
    fn create_topic(
        &self,
        world: SettlementWorld,
        rendered: ForumRenderedThread,
    ) -> ApplicationFuture<'_, ForumTopicPublication>;
}

/// 固定论坛楼层写入端口；实现必须校验 exact thread/post 身份并保持 POST 后未知结果栅栏。
pub trait ForumPostWriter: Send + Sync {
    /// 在指定主题追加一条回复；仅用于按顺序建立目录续页与系统日志。
    fn reply_topic(
        &self,
        world: SettlementWorld,
        thread_id: String,
        body: ForumRenderedPost,
    ) -> ApplicationFuture<'_, ForumPublishedPost>;

    /// 编辑指定稳定 post ID；`title`/`description` 仅在编辑 #1 时写入主题元数据。
    fn edit_post(
        &self,
        world: SettlementWorld,
        thread_id: String,
        post_id: String,
        metadata: Option<ForumTopicMetadata>,
        body: ForumRenderedPost,
    ) -> ApplicationFuture<'_, ForumPublishedPost>;

    /// 编辑稳定 post 并报告实际 POST 边界；预读或限速超时不得标记为已写入。
    fn edit_post_with_progress(
        &self,
        world: SettlementWorld,
        thread_id: String,
        post_id: String,
        metadata: Option<ForumTopicMetadata>,
        body: ForumRenderedPost,
        progress: ForumWriteProgress,
    ) -> ApplicationFuture<'_, ForumPublishedPost>;

    /// 从已校验的编辑表单读取服务端保存的原始 BBCode；只读操作允许轮换失效凭据。
    /// 启用正文校验时统一通过 [`forum_post_bodies_match`] 比较，不得用布局标记或本地版本替代。
    fn read_post_body(
        &self,
        world: SettlementWorld,
        thread_id: String,
        post_id: String,
    ) -> ApplicationFuture<'_, ForumRenderedPost>;
}

/// 将同一数据库快照转换为不含内部密钥和数据库行的论坛正文。
pub trait ForumRenderer: Send + Sync {
    fn render(
        &self,
        snapshot: ForumAuctionSnapshot,
        equipment_links: Vec<PublicEquipmentGameLink>,
    ) -> Result<ForumRenderedThread, ForumRenderError>;

    /// 渲染最后一个目录页之后的系统日志；水位只表示已经完整处理到的连续楼层。
    fn render_system_log(
        &self,
        auction_id: AuctionId,
        catalog_page_count: usize,
        processed_through_post_number: usize,
        entries: Vec<ForumSystemLogEntry>,
    ) -> Result<ForumRenderedPost, ForumRenderError>;
}

/// 系统日志中展示的有界处理结果；正文只包含论坛公开帖子与安全错误摘要。
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
