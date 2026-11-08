//! 论坛回帖扫描与竞价命令导入端口。
//! 论坛帖子只是外部输入。Worker 必须先通过身份和 lot 投影解析，再使用现有竞价命令端口；
//! 本模块不允许论坛适配器直接写拍卖状态。

use std::fmt;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use auction_domain::{AuctionId, LotId, ProxyAuthorizationId, SettlementWorld, UserId};

use crate::ports::admin::{AdminForumPreviewError, AdminForumPreviewResult};
use crate::ports::auction::ApplicationFuture;
use crate::ports::external_metrics::ForumTopicScanMode;
use crate::ports::forum::{ForumAuctionSnapshot, ForumRenderedPost, ForumRenderedThread};

/// 论坛主题分页的帖子容量；楼层号与页号的换算只在应用层定义一次。
pub const FORUM_POSTS_PER_PAGE: u32 = 20;

/// 回帖导入游标：命令账本中楼层最高的已导入帖子。
///
/// 游标只说明“导入已经走到哪一楼”，不是发送或领取证明；页号由楼层换算而不是单独持久化，
/// 因而删帖时不会出现楼层与页号互相矛盾的记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumTopicCursor {
    pub post_id: String,
    pub post_number: u32,
}

impl ForumTopicCursor {
    /// 覆盖该楼层的页号（0 基）。
    pub const fn page(&self) -> u32 {
        (self.post_number - 1) / FORUM_POSTS_PER_PAGE
    }

    /// 游标必须指向真实用户楼层（系统日志楼层之后）且带规范数字 post ID。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.post_number < 3
            || self.post_id.is_empty()
            || self.post_id.len() > 32
            || self.post_id.starts_with('0')
            || !self.post_id.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err("论坛回帖游标无效");
        }
        Ok(())
    }
}

/// 固定楼层覆盖的页数；目录续页与系统日志可能延续到首帖之后的多个分页。
pub fn forum_topic_fixed_page_count(fixed_through_post_number: u32) -> u32 {
    fixed_through_post_number.div_ceil(FORUM_POSTS_PER_PAGE)
}

/// 增量读取必须覆盖的页号（0 基、升序）：固定楼层页与游标页及其之后。
///
/// 游标缺失、无效或落在固定楼层之内时返回全部页——那正是“回退完整扫描”的分页选择，
/// 因为固定楼层之前的页既不会跳过也无法再压缩。
pub fn forum_topic_scan_pages(
    cursor_page: Option<u32>,
    fixed_page_count: u32,
    page_count: u32,
) -> Vec<u32> {
    let fixed_page_count = fixed_page_count.min(page_count);
    let Some(cursor_page) =
        cursor_page.filter(|page| *page >= fixed_page_count && *page < page_count)
    else {
        return (0..page_count).collect();
    };
    let mut pages = (0..fixed_page_count).collect::<Vec<_>>();
    pages.extend(cursor_page..page_count);
    pages
}

/// 一次主题读取的分页覆盖。
///
/// 增量扫描只包含固定楼层页与游标页之后的分页，`preview.posts` 因此可能跳层；
/// `post_numbers` 保存每条帖子在主题内的绝对楼层，供命令账本与系统日志使用。
pub struct ForumTopicScan {
    pub preview: AdminForumPreviewResult,
    pub post_numbers: Vec<u32>,
    pub mode: ForumTopicScanMode,
    /// 远端主题按 HTTP 分页发现的总页数；默认读取器不知道时为 None。
    pub page_count: Option<u32>,
    /// 该次扫描生效的远端主题页数上限；与 `page_count` 必须同时存在。
    pub page_limit: Option<u32>,
}

impl ForumTopicScan {
    /// 楼层必须与帖子一一对应；不满足时按解析失败处理，不能靠索引推测楼层。
    pub fn validate(&self) -> Result<(), AdminForumPreviewError> {
        self.preview.validate()?;
        if self.post_numbers.len() != self.preview.posts.len() {
            return Err(AdminForumPreviewError::ParseFailed);
        }
        Ok(())
    }
}

impl fmt::Debug for ForumTopicScan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ForumTopicScan")
            .field("preview", &self.preview)
            .field("post_count", &self.post_numbers.len())
            .field("first_post_number", &self.post_numbers.first())
            .field("last_post_number", &self.post_numbers.last())
            .field("mode", &self.mode)
            .finish()
    }
}

/// 领取主题时观察到的拍卖阶段；跨阶段样本不能作为严格 Final SLA 证明。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForumReplyPhase {
    Normal,
    Final,
    Unknown,
}

impl ForumReplyPhase {
    /// 只映射领取时刻已连接的拍卖状态，不新增查询。
    pub const fn from_auction_status(status: &str) -> Self {
        match status.as_bytes() {
            b"live" => Self::Normal,
            b"final" => Self::Final,
            _ => Self::Unknown,
        }
    }
}

/// 已发布论坛主题的最小扫描目标。
#[derive(Clone, PartialEq, Eq)]
pub struct ForumReplyTopic {
    pub auction_id: AuctionId,
    pub world: SettlementWorld,
    /// 领取时观察到的拍卖阶段；只用于指标分桶，不参与命令裁决。
    pub reply_phase: ForumReplyPhase,
    pub thread_id: String,
    pub catalog_post_id: String,
    pub catalog_layout: crate::ports::forum::ForumCatalogLayout,
    /// 按连续楼层排序的目录续页；发布后不得插楼或挪动已有用户回帖。
    pub catalog_continuation: Vec<ForumCatalogPage>,
    pub system_log_post_id: String,
    pub title: String,
    pub catalog_body: String,
    pub system_log_body: String,
    pub content_revision: i64,
    /// 领取时为空；Worker 仅在本轮所有固定帖确认后填入所渲染的 settled/cancelled 版本。
    /// 存储完成入口必须在拍卖锁内复核；部分进度入口不得使用它声明完成。
    pub confirmed_terminal_version: Option<i64>,
    /// 当前完整扫描的派生楼层；删帖可使它下降，历史命令楼层与事实 sequence 不随之改写。
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
            .field("reply_phase", &self.reply_phase)
            .field("thread_id", &self.thread_id)
            .field("catalog_post_id", &self.catalog_post_id)
            .field("catalog_layout", &self.catalog_layout)
            .field("catalog_continuation", &self.catalog_continuation)
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

/// 已取得远端证明的目录续页；正文可能包含公开装备 URL，调试输出必须脱敏。
#[derive(Clone, PartialEq, Eq)]
pub struct ForumCatalogPage {
    pub post_id: String,
    pub body: String,
}

impl fmt::Debug for ForumCatalogPage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ForumCatalogPage")
            .field("post_id", &self.post_id)
            .field("body", &"[REDACTED]")
            .finish()
    }
}

impl ForumReplyTopic {
    /// 目录页之后的系统日志楼层，也是尚未处理用户回帖时的安全水位。
    pub fn system_log_post_number(&self) -> usize {
        self.catalog_continuation.len() + 2
    }
}

/// 已持久化论坛命令的安全结果，用于系统日志的处理摘要。
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
    /// 首次记录后只作为历史观察事实，后续删帖引起的重新编号不改变稳定命令身份。
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

/// Worker 需要区分“已完成的原命令”和“本次可以继续执行的 pending 命令”，否则竞价已成功
/// 但通知入队失败时，下一轮只看到 `should_run = false`，会永久跳过通知重试。旧版账本使用
/// 不同的稳定 ID 算法，因此还必须返回实际命中的命令 ID，让后续竞价、通知和完成写入继续
/// 使用同一个幂等身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForumCommandStart {
    pub command_id: Uuid,
    pub status: ForumCommandStatus,
    pub should_run: bool,
    /// 本次事务首次插入账本时为 true；pending 重试与已完成命令重放均为 false。
    pub is_new: bool,
}

/// W14.4：论坛 `notify` 的结果；不再产生导入回执邮件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifySubscription {
    /// 本次新建订阅。
    Subscribed,
    /// 用户此前已在同 lot 上标记，天然去重。
    AlreadySubscribed,
    /// 超出每帖或每账号每场订阅上限；调用方写入系统日志楼的拒绝结果。
    Capped(&'static str),
}

pub trait ForumReplyStore: Send + Sync {
    /// 领取固定楼层刷新目标；内容 lease 必须覆盖单主题总时限，避免多 Worker 交叉编辑远端帖子。
    /// 必须在 LIMIT 前按实际凭据可用世界过滤，不能让不可用世界耗尽本轮名额。
    fn claim_published_topics(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        available_worlds: Vec<SettlementWorld>,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<ForumReplyTopic>>;

    /// 读取权威 live/final 状态以停止终态新回帖消费；命令开始仍须在锁内再次复核。
    fn reply_commands_open(&self, auction_id: AuctionId) -> ApplicationFuture<'_, bool>;

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

    /// 在命令事务内把回帖者的该 lot 标记为关注；按用户 × lot 唯一，天然去重。
    /// 上限在锁内重新计数：每帖最多 8 条订阅、每账号每场最多 64 条。
    fn subscribe_notify(
        &self,
        command_id: Uuid,
        auction_id: AuctionId,
        lot_id: LotId,
        user_id: UserId,
        post_id: String,
        lot_code: String,
    ) -> ApplicationFuture<'_, NotifySubscription>;

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

    /// 读取已导入命令的楼层最高游标；没有命令时返回 None（首轮必须完整扫描）。
    ///
    /// 游标只是导入位置，不证明命令已经裁决，也不改变任何 pending 阻断规则。
    fn reply_cursor(
        &self,
        auction_id: AuctionId,
        thread_id: String,
    ) -> ApplicationFuture<'_, Option<ForumTopicCursor>>;

    /// 读取该拍卖 live lot 的最早有效截止，用于 Final 阶段加快论坛导入节奏。
    fn earliest_live_lot_deadline(
        &self,
        auction_id: AuctionId,
    ) -> ApplicationFuture<'_, Option<DateTime<Utc>>>;

    /// 读取当前权威拍卖快照，用于竞价命令提交后刷新所有目录页。
    fn load_current_snapshot(
        &self,
        auction_id: AuctionId,
    ) -> ApplicationFuture<'_, ForumAuctionSnapshot>;

    /// 在所有远端固定楼层均已得到 exact post proof 后，以 revision CAS 更新本地正文证明并推进下次扫描 due。
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

    /// 记录远端主题 HTTP 分页总数；失败只影响告警观察，不回滚已完成的扫描。
    fn record_topic_page_count(
        &self,
        _auction_id: AuctionId,
        _page_count: u32,
        _page_limit: u32,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }

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

    /// 增量读取主题：固定楼层所在页 + 游标页及其之后的分页。
    ///
    /// 实现必须先核对游标页仍包含游标帖子；删帖使楼层前移或帖子缺失时回退完整读取。
    /// 默认实现忽略游标并完整读取，语义仍然正确，只是不做增量优化。
    fn fetch_topic_since(
        &self,
        world: SettlementWorld,
        thread_id: String,
        cursor: Option<ForumTopicCursor>,
        fixed_through_post_number: u32,
    ) -> ApplicationFuture<'_, ForumTopicScan> {
        let _ = (cursor, fixed_through_post_number);
        Box::pin(async move {
            let preview = self.fetch_topic(world, thread_id).await?;
            let post_numbers = (0..preview.posts.len())
                .map(|index| u32::try_from(index).unwrap_or(u32::MAX).saturating_add(2))
                .collect();
            Ok(ForumTopicScan {
                preview,
                post_numbers,
                mode: ForumTopicScanMode::Full,
                page_count: None,
                page_limit: None,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::ports::forum::reply as parent;

    fn cursor(post_number: u32) -> ForumTopicCursor {
        ForumTopicCursor { post_id: "9000000".to_owned(), post_number }
    }

    #[test]
    fn cursor_page_maps_floor_to_zero_based_page() {
        assert_eq!(cursor(3).page(), 0);
        assert_eq!(cursor(20).page(), 0);
        assert_eq!(cursor(21).page(), 1);
        assert_eq!(cursor(41).page(), 2);
    }

    #[test]
    fn cursor_requires_real_user_floor_and_numeric_id() {
        assert!(cursor(3).validate().is_ok());
        assert!(ForumTopicCursor { post_id: "01".to_owned(), post_number: 5 }.validate().is_err());
        assert!(ForumTopicCursor { post_id: "a1".to_owned(), post_number: 5 }.validate().is_err());
        assert!(ForumTopicCursor { post_id: "123".to_owned(), post_number: 2 }.validate().is_err());
    }

    #[test]
    fn fixed_page_count_covers_system_log_floor() {
        assert_eq!(forum_topic_fixed_page_count(2), 1);
        assert_eq!(forum_topic_fixed_page_count(20), 1);
        assert_eq!(forum_topic_fixed_page_count(21), 2);
        assert_eq!(forum_topic_fixed_page_count(45), 3);
    }

    #[test]
    fn incremental_pages_skip_only_pages_before_cursor() {
        // 10 页主题，固定楼层占 1 页，游标在第 6 页（0 基 5）。
        assert_eq!(forum_topic_scan_pages(Some(5), 1, 10), vec![0, 5, 6, 7, 8, 9]);
        // 游标页与固定页相同，没有可跳过的页。
        assert_eq!(forum_topic_scan_pages(Some(0), 1, 3), vec![0, 1, 2]);
    }

    #[test]
    fn cursor_advance_narrows_page_window_after_new_import() {
        // 5 页主题、固定楼层 1 页：新命令落在第 21 楼时从第 1 页开始增量读取。
        let first_import = cursor(21);
        assert_eq!(forum_topic_scan_pages(Some(first_import.page()), 1, 5), vec![0, 1, 2, 3, 4]);
        // 命令继续推进到第 41 楼后，第 1 页也被跳过。
        let later_import = cursor(41);
        assert_eq!(forum_topic_scan_pages(Some(later_import.page()), 1, 5), vec![0, 2, 3, 4]);
        // 楼层推进但仍在同一页时不重复缩小窗口，分页边界决定预算。
        let same_page = cursor(45);
        assert_eq!(same_page.page(), later_import.page());
    }

    #[test]
    fn missing_or_fixed_region_cursor_falls_back_to_all_pages() {
        assert_eq!(forum_topic_scan_pages(None, 2, 4), vec![0, 1, 2, 3]);
        // 游标落在固定楼层页之内时整场读取，避免目录续页与系统日志缺失。
        assert_eq!(forum_topic_scan_pages(Some(1), 2, 4), vec![0, 1, 2, 3]);
        // 游标页超出已知页数（主题变短）同样回退完整读取。
        assert_eq!(forum_topic_scan_pages(Some(9), 1, 3), vec![0, 1, 2]);
        // 固定页数超过实际页数时压缩到实际页数。
        assert_eq!(forum_topic_scan_pages(Some(2), 5, 2), vec![0, 1]);
    }
}
