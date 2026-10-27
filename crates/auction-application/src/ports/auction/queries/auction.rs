//! Public 拍卖目录和当前拍卖读取端口。

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::ApplicationFuture;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicAuctionListRequest {
    pub world_id: Option<String>,
    pub status: String,
    pub limit: i64,
    pub offset: i64,
    pub cursor: Option<PublicAuctionCursor>,
    /// 仅新版本目录需要主题链接；旧版本不读取论坛同步记录。
    pub include_forum_thread_url: bool,
}

/// Public 拍卖列表的稳定 keyset 游标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicAuctionCursor {
    Start,
    Boundary { sequence: i64, id: Uuid },
}

/// 不含内部策略、卖家身份和数据库字段的公开拍卖摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicAuctionSummary {
    pub id: Uuid,
    pub world_id: String,
    pub sequence: i64,
    pub status: String,
    pub scheduled_open_at: DateTime<Utc>,
    pub opened_at: Option<DateTime<Utc>>,
    pub final_phase_at: Option<DateTime<Utc>>,
    pub effective_end_at: Option<DateTime<Utc>>,
    pub settled_at: Option<DateTime<Utc>>,
    pub lot_count: i64,
    pub sold_count: i64,
    pub unsold_count: i64,
    pub sold_amount: i64,
    pub state_version: i64,
    /// 按请求选择且已通过内容证明与 HTTPS 校验的论坛链接，不包含镜像正文。
    pub forum_link: Option<crate::PublicForumLink>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicAuctionPage {
    pub entries: Vec<PublicAuctionSummary>,
    pub total: Option<i64>,
    pub limit: i64,
    pub offset: i64,
    pub next_cursor: Option<PublicAuctionCursor>,
    pub has_more: bool,
}

/// 当前公开 projection 的最小 HTTP 安全投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentAuctionSnapshot {
    pub id: Uuid,
    pub world_id: String,
    pub sequence: i64,
    pub status: String,
    pub opens_at: DateTime<Utc>,
    pub scheduled_ends_at: Option<DateTime<Utc>>,
    pub presentation_mode: String,
    pub bidding_open: bool,
    pub preview_revision: i64,
}

impl CurrentAuctionSnapshot {
    /// 校验公开快照的基础字段以及状态、展示模式和竞价能力之间的合同。
    /// 该校验只覆盖所有 Public 读取都能表达的核心不变量；终阶段的
    /// `FinalRestriction` 和 ForumSync 结果不属于当前快照，因此不在这里推断。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.id.is_nil()
            || self.world_id.is_empty()
            || self.world_id.len() > 64
            || self.world_id != self.world_id.trim()
            || self.world_id.chars().any(char::is_control)
            || self.sequence <= 0
            || is_go_zero_time(self.opens_at)
            || self.preview_revision < 0
        {
            return Err("公开当前拍卖快照无效");
        }

        match self.status.as_str() {
            "preview" => {
                if !matches!(self.presentation_mode.as_str(), "preview" | "opening")
                    || self.bidding_open
                    || self.scheduled_ends_at.is_some()
                {
                    return Err("公开预览拍卖快照无效");
                }
            }
            "live" | "final" => {
                if self.presentation_mode != "live" || !self.bidding_open {
                    return Err("公开竞价拍卖快照无效");
                }
            }
            "settling" | "settled" => {
                if self.presentation_mode != "live" || self.bidding_open {
                    return Err("公开收口拍卖快照无效");
                }
            }
            _ => return Err("公开拍卖快照状态无效"),
        }

        if self.scheduled_ends_at.is_some_and(|value| value < self.opens_at) {
            return Err("公开拍卖快照时间关系无效");
        }
        Ok(())
    }
}

/// 单拍卖 Public WebSocket 预检可观察的公开状态闭集。
/// 存储端只为启用世界中的公开 projection 返回这些状态；HTTP 连接边界据此拒绝新的
/// `Settled` 连接，但不会据事件历史猜测拍卖是否仍可订阅。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicRealtimeAuctionStatus {
    Preview,
    Live,
    Final,
    Settling,
    Settled,
}

fn is_go_zero_time(value: DateTime<Utc>) -> bool {
    value.timestamp() == -62_135_596_800 && value.timestamp_subsec_nanos() == 0
}

/// Public 拍卖只读端口；具体 SQL 和后端时间/UUID 表示由存储适配器负责。
pub trait AuctionReader: Send + Sync {
    fn list_public_auctions(
        &self,
        request: PublicAuctionListRequest,
    ) -> ApplicationFuture<'_, PublicAuctionPage>;

    fn current_auction(
        &self,
        world_id: String,
    ) -> ApplicationFuture<'_, Option<CurrentAuctionSnapshot>>;

    /// 隐藏草稿、取消场次和关闭世界的活动场次；预览遵循当前公开 projection 边界。
    fn public_auction_countdown(
        &self,
        auction_id: Uuid,
    ) -> ApplicationFuture<'_, Option<super::PublicAuctionCountdown>>;

    fn public_realtime_auction_status(
        &self,
        auction_id: Uuid,
    ) -> ApplicationFuture<'_, Option<PublicRealtimeAuctionStatus>>;
}
