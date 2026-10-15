use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use auction_domain::AuctionId;

use super::ApplicationFuture;

/// Admin 结算报告中的拍卖生命周期快照。
///
/// 这是稳定报告所需的只读投影，不复用领域 `Auction` 聚合，避免把数据库的管理字段或
/// 兼容字段反向塞入状态机。字段必须来自同一数据库快照。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettlementAuctionSnapshot {
    pub id: AuctionId,
    pub world_id: String,
    pub sequence: i64,
    pub status: String,
    pub preview_scheduled_at: DateTime<Utc>,
    pub scheduled_open_at: DateTime<Utc>,
    pub preview_opened_at: Option<DateTime<Utc>>,
    pub preview_manual_open: bool,
    pub preview_revision: i64,
    pub opened_at: Option<DateTime<Utc>>,
    pub final_phase_at: Option<DateTime<Utc>>,
    pub settled_at: Option<DateTime<Utc>>,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub cancelled_reason: Option<String>,
    pub cancelled_by: Option<String>,
    pub state_version: i64,
    pub scheduled_end_at: Option<DateTime<Utc>>,
    pub effective_end_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 结算报告中允许公开的 adjustment metadata。
#[derive(Debug, Clone)]
pub struct SettlementAdjustment {
    pub policy: Option<String>,
}

/// Admin 结算摘要。
#[derive(Debug, Clone)]
pub struct SettlementSummary {
    pub opening_balance: i64,
    pub tax_income: i64,
    pub discount_demand: i64,
    pub compensation_out: i64,
    pub closing_balance: i64,
    pub movement_count: i64,
    pub settled_at: DateTime<Utc>,
}

/// Admin 结算报告中的单个 lot 金额投影。
#[derive(Debug, Clone)]
pub struct SettlementLotReport {
    pub id: auction_domain::LotId,
    pub lot_number: i32,
    pub status: String,
    pub equipment_name: String,
    pub equipment_category: String,
    pub seller: Option<String>,
    pub winner: Option<String>,
    pub final_price: i64,
    pub listed_price: i64,
    pub buyer_amount: i64,
    pub seller_amount: i64,
    pub tax_amount: i64,
    pub compensation_amount: i64,
    pub hold_ppm: i32,
    pub adjustment_ppm: i32,
    pub adjustment: SettlementAdjustment,
}

/// 一致的 Admin 结算报告快照。
#[derive(Debug, Clone)]
pub struct SettlementReport {
    pub auction: SettlementAuctionSnapshot,
    pub settlement: Option<SettlementSummary>,
    pub lots: Vec<SettlementLotReport>,
}

/// Admin 结算报告读取端口。
///
/// 在同一数据库快照读取结算摘要与 lot 明细，不以空报告掩盖读取失败。
pub trait SettlementReportReader: Send + Sync {
    fn settlement_report(
        &self,
        auction_id: AuctionId,
    ) -> ApplicationFuture<'_, Option<SettlementReport>>;
}
