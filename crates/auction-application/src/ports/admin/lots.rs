//! Admin 拍卖 lot 读取端口。
//!
//! 这里定义管理端需要的安全 lot 快照。装备密钥、代理上限和原始外部正文不属于该投影；
//! SQL、后端时间表示与一致性事务留在 `auction-storage`。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use auction_domain::{AuctionId, EquipmentId, LotId, ProxyAuthorizationId, SubmissionId, UserId};

use super::{ApplicationFuture, SettlementAuctionSnapshot};

/// Admin lot 列表允许的最大单页大小。
pub const MAX_ADMIN_LOT_PAGE_SIZE: i64 = 100;

/// Admin lot 列表允许的最大 offset，避免通过管理接口执行无界深分页扫描。
pub const MAX_ADMIN_LOT_OFFSET: i64 = 100_000;

/// Admin lot 列表查询条件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdminLotListRequest {
    pub auction_id: AuctionId,
    pub limit: i64,
    pub offset: i64,
}

impl AdminLotListRequest {
    /// 校验拍卖标识和有界分页。
    pub fn validate(self) -> bool {
        !self.auction_id.is_nil()
            && self.limit > 0
            && self.limit <= MAX_ADMIN_LOT_PAGE_SIZE
            && self.offset >= 0
            && self.offset <= MAX_ADMIN_LOT_OFFSET
    }
}

/// Admin 可见的 lot、装备和投稿来源安全快照。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminLotSummary {
    pub id: LotId,
    pub auction_id: AuctionId,
    pub equipment_id: EquipmentId,
    pub submission_id: Option<SubmissionId>,
    pub lot_number: i32,
    pub status: String,
    pub start_price: i64,
    pub current_price: i64,
    pub leader_user_id: Option<UserId>,
    pub effective_end_at: Option<DateTime<Utc>>,
    pub slow_due_at: Option<DateTime<Utc>>,
    pub slow_generation: i64,
    pub slow_authorization_id: Option<ProxyAuthorizationId>,
    pub state_version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub external_equipment_id: String,
    pub equipment_name: String,
    pub equipment_category: String,
    pub equipment_attributes: Value,
    pub equipment_collected_at: DateTime<Utc>,
    pub seller: Option<String>,
    pub submission_source_kind: Option<String>,
    pub submission_source_reference: Option<String>,
    pub mailbox_key: Option<String>,
    pub external_mail_id: Option<String>,
    pub submission_received_at: Option<DateTime<Utc>>,
}

impl AdminLotSummary {
    /// 校验跨表投影、来源闭集和慢速代理状态的形状。
    pub fn validate(&self) -> bool {
        if self.id.is_nil()
            || self.auction_id.is_nil()
            || self.equipment_id.is_nil()
            || self.submission_id.is_some_and(SubmissionId::is_nil)
            || self.leader_user_id.is_some_and(UserId::is_nil)
            || self.slow_authorization_id.is_some_and(ProxyAuthorizationId::is_nil)
            || !matches!(self.status.as_str(), "pending" | "live" | "sold" | "unsold" | "withdrawn")
            || self.lot_number <= 0
            || self.start_price <= 0
            || self.current_price < self.start_price
            || self.slow_generation < 0
            || self.state_version <= 0
            || self.created_at > self.updated_at
            || !valid_text(&self.external_equipment_id, 128)
            || !valid_text(&self.equipment_name, 512)
            || !valid_text(&self.equipment_category, 128)
            || !self.equipment_attributes.is_object()
            || self.equipment_collected_at.timestamp_micros() == 0
            || !valid_optional_text(self.seller.as_deref(), 256)
            || !valid_optional_text(self.submission_source_kind.as_deref(), 32)
            || !valid_optional_text(self.submission_source_reference.as_deref(), 2048)
            || !valid_optional_text(self.mailbox_key.as_deref(), 256)
            || !valid_optional_text(self.external_mail_id.as_deref(), 256)
        {
            return false;
        }
        if matches!(self.status.as_str(), "live" | "sold") && self.effective_end_at.is_none() {
            return false;
        }
        if self.slow_due_at.is_some() != self.slow_authorization_id.is_some()
            || self.slow_due_at.is_some_and(|_| self.status != "live" || self.slow_generation <= 0)
        {
            return false;
        }
        if self.submission_id.is_none() {
            return self.seller.is_none()
                && self.submission_source_kind.is_none()
                && self.submission_source_reference.is_none()
                && self.mailbox_key.is_none()
                && self.external_mail_id.is_none()
                && self.submission_received_at.is_none();
        }
        let (Some(seller), Some(source_kind), Some(received_at)) = (
            self.seller.as_deref(),
            self.submission_source_kind.as_deref(),
            self.submission_received_at,
        ) else {
            return false;
        };
        if !valid_text(seller, 256) || received_at.timestamp_micros() == 0 {
            return false;
        }
        match source_kind {
            "mail" => {
                self.submission_source_reference.is_none()
                    && self.mailbox_key.is_some()
                    && self.external_mail_id.is_some()
            }
            "admin" => {
                self.submission_source_reference.is_some()
                    && self.mailbox_key.is_none()
                    && self.external_mail_id.is_none()
            }
            _ => false,
        }
    }
}

/// Admin lot 分页结果；拍卖与 lot 必须来自同一数据库快照。
#[derive(Debug, Clone)]
pub struct AdminLotPage {
    pub auction: SettlementAuctionSnapshot,
    pub entries: Vec<AdminLotSummary>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

impl AdminLotPage {
    /// 校验分页边界、拍卖归属和安全投影。
    pub fn validate(&self, request: AdminLotListRequest) -> bool {
        request.validate()
            && self.auction.id.as_uuid() == request.auction_id.as_uuid()
            && self.total >= 0
            && self.limit == request.limit
            && self.offset == request.offset
            && self.entries.len() as i64 <= self.limit
            && self.entries.iter().all(|entry| {
                entry.auction_id == request.auction_id.as_uuid().into() && entry.validate()
            })
    }
}

/// Admin lot 读取窄端口。
pub trait AdminLotReader: Send + Sync {
    fn list_admin_lots(
        &self,
        request: AdminLotListRequest,
    ) -> ApplicationFuture<'_, Option<AdminLotPage>>;
}

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn valid_optional_text(value: Option<&str>, maximum: usize) -> bool {
    value.is_none_or(|value| valid_text(value, maximum))
}
