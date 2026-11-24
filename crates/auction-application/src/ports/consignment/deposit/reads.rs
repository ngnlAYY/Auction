//! v2 status、本人寄件指引与 Admin 隔离详情读取端口。

use std::fmt;

use auction_domain::SettlementWorld;
use auction_domain::UserId;
use auction_domain::consignment::deposit::{
    EnvelopeItemKind, LegacyAuctionOwner, QuarantineReason, ReleaseKind, TokenText,
};
use auction_domain::consignment::{FeeBps, ListingMode, ListingPrice, SeasonState};
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::consignment::deposit::commands::ListingStateChange;
use crate::ports::consignment::errors::ConsignmentFuture;

/// 一个世界的 v2 收件与购买能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepositWorldStatus {
    /// 能处理该世界、该卖家的系统 RTS。
    pub mail_ready: bool,
    /// 购买能力；第 4 期固定为 false。
    pub purchase_ready: bool,
    /// 季态。
    pub season_state: SeasonState,
    /// 预停时刻；main 或未知时为 `None`。
    pub cutoff_at: Option<Timestamp>,
    /// 季末时刻；main 或未知时为 `None`。
    pub season_end_at: Option<Timestamp>,
}

/// 两世界的固定键对象。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepositWorlds {
    /// main 世界。
    pub main: DepositWorldStatus,
    /// isekai 世界。
    pub isekai: DepositWorldStatus,
}

/// `GET /api/v2/consignment/status` 的应用投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepositStatus {
    /// 总开关。
    pub enabled: bool,
    /// 登记解析能力就绪。
    pub registration_ready: bool,
    /// 登记模式。
    pub registration_modes: Vec<ListingMode>,
    /// 允许登记的世界。
    pub registration_worlds: Vec<SettlementWorld>,
    /// 当前费率；第 4 期只展示。
    pub fee_bps: FeeBps,
    /// 标价下限。
    pub min_price: ListingPrice,
    /// 标价上限。
    pub max_price: ListingPrice,
    /// 至少一个世界可处理系统 RTS。
    pub accepts_mail: bool,
    /// 至少一个世界可购买。
    pub purchasable: bool,
    /// 分世界能力。
    pub worlds: DepositWorlds,
}

/// 本人当前有效的寄件指引。
#[derive(Clone, PartialEq, Eq)]
pub struct ActiveDepositInstructions {
    /// 世界。
    pub world: SettlementWorld,
    /// 认证 Bot 的显示名；不是认证 UID。
    pub bot: String,
    /// 主题，固定 `consign`。
    pub subject: String,
    /// 仅发给本人的完整令牌正文；受保护类型，不实现序列化。
    pub body: TokenText,
    /// 预约 UUID。
    pub reservation_id: Uuid,
    /// 预约截止。
    pub expires_at: Timestamp,
}

impl fmt::Debug for ActiveDepositInstructions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActiveDepositInstructions")
            .field("world", &self.world)
            .field("bot", &self.bot)
            .field("subject", &self.subject)
            .field("body", &"[REDACTED]")
            .field("reservation_id", &self.reservation_id)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// 本人寄件指引。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedDepositInstructions {
    /// 没有有效预约；不返回任何秘密。
    None,
    /// 当前有效预约。
    Active(ActiveDepositInstructions),
}

/// 隔离状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuarantineState {
    /// 仍活动。
    Active,
    /// 已解除。
    Released,
}

impl QuarantineState {
    /// 稳定 wire 码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Released => "released",
        }
    }
}

/// Admin 可执行的隔离动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdminDepositAction {
    /// 排队重检。
    Recheck,
    /// 引用新观察解除。
    ReleaseQuarantine,
}

impl AdminDepositAction {
    /// 稳定 wire 码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Recheck => "recheck",
            Self::ReleaseQuarantine => "release",
        }
    }
}

/// 观察的安全附件投影；不含原文 EID、凭证或正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminObservationItem {
    /// 原序号。
    pub ordinal: u32,
    /// 附件种类。
    pub kind: EnvelopeItemKind,
    /// 规范 EID；无法读取时为空。
    pub canonical_eid: Option<String>,
    /// Credits/COD 项的金额；其他种类为空。
    pub amount: Option<i64>,
}

/// Admin 可见的一次观察。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminQuarantineObservation {
    /// 观察 UUID。
    pub observation_id: Uuid,
    /// 观察时刻。
    pub observed_at: Timestamp,
    /// 解析器版本。
    pub parser_version: String,
    /// 清单摘要。
    pub digest: [u8; 32],
    /// 观察到的 Credits 金额。
    pub credits: Option<i64>,
    /// 观察到的 COD 金额。
    pub cod: Option<i64>,
    /// 是否可原样退回。
    pub returnable: Option<bool>,
    /// 是否可取得。
    pub takeable: Option<bool>,
    /// 脱敏后的附件投影。
    pub items: Vec<AdminObservationItem>,
}

/// 已解除后的最终决定投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminFinalDecision {
    /// 解除事实 UUID。
    pub release_id: Uuid,
    /// 最终决定 UUID。
    pub decision_id: Uuid,
    /// 解除结果。
    pub kind: ReleaseKind,
    /// 原 owner 引用；寄售内部结果为空。
    pub owner: Option<LegacyAuctionOwner>,
    /// 解除时刻。
    pub released_at: Timestamp,
    /// 整封接受的逐件 L。
    pub listing_changes: Vec<ListingStateChange>,
    /// 旧拍卖退件案件的义务 UUID；E2 允许为空。
    pub obligation_id: Option<Uuid>,
    /// 原退件邮件 UUID。
    pub return_mail_id: Option<Uuid>,
    /// 原退件证据 UUID。
    pub return_evidence_id: Option<Uuid>,
    /// 原恢复记录 UUID。
    pub recovery_id: Option<Uuid>,
    /// 恢复接管事实 UUID。
    pub recovery_handoff_id: Option<Uuid>,
    /// 第 6 期寄售退件接管 UUID。
    pub return_intake_id: Option<Uuid>,
    /// 第 6 期原轮次。
    pub original_round: Option<u32>,
    /// 第 6 期 Take 工作 UUID。
    pub take_work_id: Option<Uuid>,
    /// 第 6 期 Take 投递 UUID。
    pub take_delivery_id: Option<Uuid>,
}

/// `GET /api/v2/admin/consignment/deposits/{id}` 的查询。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdminDepositQuery {
    /// 隔离 UUID。
    pub quarantine_id: Uuid,
    /// 观察分页游标。
    pub cursor: Option<Uuid>,
    /// 观察页大小；调用方须限制在公共分页预算内。
    pub limit: u16,
}

/// Admin 隔离详情。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminDepositDetail {
    /// 隔离 UUID。
    pub quarantine_id: Uuid,
    /// 隔离版本。
    pub version: u64,
    /// 隔离状态。
    pub state: QuarantineState,
    /// 来源世界。
    pub world: SettlementWorld,
    /// 外部 MID。
    pub external_mid: String,
    /// 首次决定 UUID。
    pub first_decision_id: Uuid,
    /// 隔离原因。
    pub reason: QuarantineReason,
    /// 累计观察总数；`observations` 只是当前页。
    pub observation_total: u64,
    /// 观察页。
    pub observations: Vec<AdminQuarantineObservation>,
    /// 允许动作。
    pub allowed_actions: Vec<AdminDepositAction>,
    /// 已解除后的最终决定。
    pub final_decision: Option<AdminFinalDecision>,
}

/// v2 状态、本人指引与 Admin 隔离读取端口。
pub trait DepositRead: Send + Sync {
    /// 汇总各世界真实收件能力；第 4 期 `purchase_ready` 固定 false。
    fn status(&self) -> ConsignmentFuture<'_, DepositStatus>;

    /// 仅返回本人当前有效预约；非本人统一没有结果。
    fn instructions(
        &self,
        seller: UserId,
        listing_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<OwnedDepositInstructions>>;

    /// 只返回安全投影、观察及原 owner 引用。
    fn quarantine(
        &self,
        query: AdminDepositQuery,
    ) -> ConsignmentFuture<'_, Option<AdminDepositDetail>>;
}

#[cfg(test)]
mod tests {
    use auction_domain::SettlementWorld;
    use auction_domain::consignment::deposit::format_token;
    use uuid::Uuid;

    use super::ActiveDepositInstructions;

    #[test]
    fn active_instructions_debug_redacts_the_reservation_body() {
        let token = format_token([0x5a; 32]);
        let instructions = ActiveDepositInstructions {
            world: SettlementWorld::Main,
            bot: "FakeBot".to_owned(),
            subject: "consign".to_owned(),
            body: token.clone(),
            reservation_id: Uuid::from_u128(41),
            expires_at: auction_time::Timestamp::UNIX_EPOCH + auction_time::Duration::hours(1),
        };
        let debug = format!("{instructions:?}");
        assert!(!debug.contains(token.as_str()));
        assert!(debug.contains("[REDACTED]"));
    }
}
