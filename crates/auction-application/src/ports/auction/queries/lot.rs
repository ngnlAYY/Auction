//! Public lot 目录、装备投影和 projection 读取端口。

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use crate::ApplicationFuture;

/// 公开 lot 读取允许的最大页大小。
pub const MAX_PUBLIC_LOT_PAGE_SIZE: i64 = 200;

/// 公开 lot 读取允许的最大 legacy offset，防止深分页拖垮数据库。
pub const MAX_PUBLIC_LOT_OFFSET: i64 = 10_000;

/// 公开 lot 分类对应的稳定 JSON key。
pub const PUBLIC_LOT_CATEGORIES: [&str; 8] =
    ["all", "one", "two", "sta", "shd", "clo", "lig", "hea"];

/// 公开 lot 列表的分类与页参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicLotListRequest {
    pub category: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

impl PublicLotListRequest {
    /// 在应用端口边界拒绝未定义分类、无界页大小和深 offset。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.limit < 1 || self.limit > MAX_PUBLIC_LOT_PAGE_SIZE || self.offset < 0 {
            return Err("公开 lot 分页参数无效");
        }
        if self.offset > MAX_PUBLIC_LOT_OFFSET {
            return Err("公开 lot 分页 offset 超出上限");
        }
        if self
            .category
            .as_deref()
            .is_some_and(|category| !PUBLIC_LOT_CATEGORIES[1..].contains(&category))
        {
            return Err("公开 lot 分类参数无效");
        }
        Ok(())
    }
}

/// Public lot 列表与详情共用的公开拍卖投影。
#[derive(Debug, Clone)]
pub struct PublicAuctionProjection {
    pub auction_id: Uuid,
    pub world_id: String,
    pub sequence: i64,
    pub auction_status: String,
    pub projection_kind: String,
    pub presentation_mode: String,
    pub bidding_open: bool,
    pub opens_at: DateTime<Utc>,
    pub scheduled_ends_at: Option<DateTime<Utc>>,
    pub preview_revision: i64,
    pub final_restriction: Option<PublicFinalRestriction>,
}

/// 终阶段对代理操作的公开限制说明。
#[derive(Debug, Clone, Copy)]
pub struct PublicFinalRestriction {
    pub new_slow_proxy_allowed: bool,
    pub existing_slow_proxy_converted: bool,
}

/// 公开装备 stat 的渲染字段。
#[derive(Debug, Clone)]
pub struct PublicEquipmentStat {
    pub value: String,
    pub base: Option<f64>,
    pub roll: Option<f64>,
}

/// 不包含 key、邮件和托管信息的公开装备投影。
#[derive(Debug, Clone)]
pub struct PublicEquipment {
    pub name: String,
    pub category: String,
    pub level: Option<i64>,
    pub info_summary: String,
    pub stats: BTreeMap<String, PublicEquipmentStat>,
    pub avg_roll: Option<f64>,
    pub game_equipment_url: String,
    pub attributes: Value,
}

/// 正式竞拍 lot 的公开摘要。
#[derive(Debug, Clone)]
pub struct PublicLiveLotSummary {
    pub id: Uuid,
    pub auction_id: Uuid,
    pub lot_number: i64,
    pub status: String,
    pub seller_name: String,
    pub start_price: i64,
    pub current_price: i64,
    pub next_bid: i64,
    pub has_leader: bool,
    pub leader_name: Option<String>,
    pub effective_end_at: Option<DateTime<Utc>>,
    pub state_version: i64,
    pub equipment: PublicEquipment,
}

/// 正式竞拍 lot 详情；attributes 只在详情分支通过 HTTP 映射公开。
#[derive(Debug, Clone)]
pub struct PublicLiveLotDetail {
    pub summary: PublicLiveLotSummary,
    pub equipment_attributes: Value,
}

/// 预览 lot 的公开摘要。
#[derive(Debug, Clone)]
pub struct PublicPreviewLotSummary {
    pub id: Uuid,
    pub auction_id: Uuid,
    pub code: String,
    pub status: String,
    pub seller_name: String,
    pub start_price: i64,
    pub equipment_name: String,
    pub equipment_category: String,
    pub equipment_level: Option<i64>,
    pub avg_roll: Option<f64>,
    pub game_equipment_url: String,
}

/// 预览 lot 详情。
#[derive(Debug, Clone)]
pub struct PublicPreviewLotDetail {
    pub summary: PublicPreviewLotSummary,
    pub equipment: PublicEquipment,
}

/// 当前公开 projection 下的单个 lot 详情。
#[derive(Debug, Clone)]
pub struct PublicLotDetail {
    pub projection: PublicAuctionProjection,
    pub preview: Option<PublicPreviewLotDetail>,
    pub live: Option<PublicLiveLotDetail>,
}

/// 当前公开 projection 下的 lot 分页。
#[derive(Debug, Clone)]
pub struct PublicLotPage {
    pub projection: PublicAuctionProjection,
    pub preview: Vec<PublicPreviewLotSummary>,
    pub live: Vec<PublicLiveLotSummary>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
    pub category_counts: BTreeMap<String, i64>,
}

/// Public lot 只读端口；未出现在当前公开 projection 的 lot 必须返回 None。
pub trait PublicLotReader: Send + Sync {
    fn list_public_lots(
        &self,
        auction_id: Uuid,
        request: PublicLotListRequest,
    ) -> ApplicationFuture<'_, Option<PublicLotPage>>;

    fn public_lot(&self, lot_id: Uuid) -> ApplicationFuture<'_, Option<PublicLotDetail>>;
}
