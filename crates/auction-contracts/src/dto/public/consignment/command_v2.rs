//! v2 本人寄售命令回执查询的契约（D9 §6.1、§6.3）。
//!
//! 成功回执与原写响应的 `data` 逐字段相同，直接复用各命令已有的 v2 回执 DTO；确定拒绝只给
//! 安全码、命令 ID 与首次 HTTP 状态（D9 §6.2），不含目标、版本、价格或任何私有字段（不变量 7）。

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::dto::public::consignment::deposit::DepositReservationReceipt;
use crate::dto::public::consignment::listing_v2::WithdrawReceiptV2;
use crate::dto::public::consignment::purchase_v2::PurchaseReceiptV2;
use crate::dto::public::consignment::recovery_v2::RelistReceiptV2;
use crate::dto::public::consignment::wire::json_object;
use crate::dto::public::consignment::wire::uuid;
use crate::dto::public::consignment::wire_v2::string_enum;

string_enum! {
    /// 可经本人回执查询的 v2 写命令。
    pub enum OwnedCommandOperationV2 {
        /// 买家购买。
        Purchase => "purchase",
        /// 卖家重列。
        Relist => "relist",
        /// 卖家 v2 撤回。
        Withdraw => "withdraw",
        /// 卖家签发寄件预约。
        IssueReservation => "issue_reservation",
    }
}

string_enum! {
    /// 确定拒绝回执的处置固定值。
    pub enum RejectedDispositionV2 {
        /// 首次请求被确定拒绝。
        Rejected => "rejected",
    }
}

string_enum! {
    /// 本人 v2 写命令入账的确定拒绝码闭集（D9 §6.2 的购买、重列、撤回与预约行）。
    pub enum OwnedCommandRejectionCodeV2 {
        /// 挂单版本已过期。
        StaleVersion => "stale_version",
        /// 标价与预期不符。
        PriceChanged => "price_changed",
        /// 挂单已不接受该动作。
        ListingNotActive => "listing_not_active",
        /// 装备已被承诺。
        EquipmentCommitted => "equipment_committed",
        /// 代管取得或持有链证据不足。
        CustodyUnproven => "custody_unproven",
        /// 运营止售有效。
        OperationsHold => "operations_hold",
        /// 季界已预停或关闭。
        SeasonClosed => "season_closed",
        /// 买家与卖家是同一本地账号。
        SelfPurchase => "self_purchase",
        /// 认证 Bot 不能作为当事人。
        BotParty => "bot_party",
        /// 当事人不具备资格。
        ParticipantIneligible => "participant_ineligible",
        /// 确定写前已知资金能力不可用。
        FundingUnavailable => "funding_unavailable",
        /// 确定写前已知邮箱能力不可用。
        MailUnavailable => "mail_unavailable",
        /// 装备已被占用。
        EquipmentBusy => "equipment_busy",
        /// 挂单额度已满。
        ListingLimitReached => "listing_limit_reached",
        /// 动作不被允许。
        ActionNotAllowed => "action_not_allowed",
    }
}

json_object! {
    ()
    /// 已入账的确定拒绝：只含安全码、命令 ID 与首次 HTTP 状态。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct RejectedCommandV2 {
        /// 固定 v2。
        pub schema_version: u16,
        pub operation: OwnedCommandOperationV2,
        pub disposition: RejectedDispositionV2,
        #[serde(deserialize_with = "uuid")]
        pub command_id: Uuid,
        pub code: OwnedCommandRejectionCodeV2,
        /// 首次响应的 HTTP 状态。
        pub http_status: i64,
    } => validate
}

impl RejectedCommandV2 {
    fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != 2 || self.command_id.is_nil() {
            return Err("拒绝回执的版本或命令标识不一致");
        }
        // 确定拒绝只可能是 4xx，或确定写前登记过的可信 503。
        if !(400..500).contains(&self.http_status) && self.http_status != 503 {
            return Err("拒绝回执的 HTTP 状态不是确定拒绝");
        }
        Ok(())
    }
}

/// `GET /api/v2/me/consignment/commands/{command_id}` 的 `data`：原成功回执或确定拒绝。
///
/// 各变体的 `operation`/`disposition` 字面量互不相同且都拒绝未知字段，按顺序解码不会歧义。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OwnedCommandReceiptV2 {
    /// `201 purchased`。
    Purchase(PurchaseReceiptV2),
    /// `200 relisted`。
    Relist(RelistReceiptV2),
    /// v2 撤回的三种成功回执。
    Withdraw(WithdrawReceiptV2),
    /// `201 reservation_issued`。
    IssueReservation(DepositReservationReceipt),
    /// 已入账的确定拒绝。
    Rejected(RejectedCommandV2),
}
