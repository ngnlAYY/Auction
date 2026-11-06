// W14.3 玩家通知扫描端口：卖方成交汇总与换季前 isekai 领取/交付提醒。
//
// 存储只返回已持久化事实的只读投影；聚合、正文冻结、稳定身份与失效判定都在应用层，
// 因此扫描可以重复执行而不会改写任何冻结结算或投递载荷。通知真正入队仍经
// `MailDeliveryEnqueuer`，与既有 outbox 共用同一幂等与审计路径。

use chrono::{DateTime, Utc};
use thiserror::Error;
use uuid::Uuid;

use crate::ports::auction::ApplicationFuture;

/// 玩家通知扫描失败原因；消息不回显收件人或正文。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum PlayerNotificationError {
    #[error("卖方汇总事实无效")]
    InvalidSellerSummary,
    #[error("换季提醒事实无效")]
    InvalidSeasonReminder,
    #[error("玩家通知身份或正文生成失败")]
    Unavailable,
}

/// 已结算场次中的单个 lot 事实；同一卖方的多行由应用层合并为一封汇总。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SellerSummaryLotFact {
    pub auction_id: Uuid,
    pub world: String,
    pub sequence: i64,
    /// 投稿时冻结的卖方游戏名；通知收件人也是该名称。
    pub seller: String,
    pub lot_number: u32,
    pub category: String,
    pub equipment_name: String,
    /// `None` 表示流拍；`Some` 为最终成交价。
    pub sold_price: Option<i64>,
}

/// 换季前仍未完成的 isekai 结算义务事实；按收件人合并为一封提醒。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeasonReminderFact {
    /// 已冻结的赛季边界（`auctions.intake_opens_at`），不按当前配置重算。
    pub season_boundary: DateTime<Utc>,
    pub recipient: String,
    pub obligation_kind: String,
    pub lot_number: u32,
    pub category: String,
    pub equipment_name: String,
    pub amount: i64,
}

/// 玩家通知事实的只读扫描端口。
pub trait PlayerNotificationScanner: Send + Sync {
    /// 返回 `settled_since` 之后结算、且至少有一个卖方尚无汇总通知的 lot 事实。
    ///
    /// 历史场次不回填：早于窗口的场次不产生新通知。
    fn seller_summary_facts(
        &self,
        settled_since: DateTime<Utc>,
        limit: u32,
    ) -> ApplicationFuture<'_, Vec<SellerSummaryLotFact>>;

    /// 返回即将到来的冻结赛季边界之前、仍有未完成 isekai 义务的收件人事实。
    ///
    /// 只使用已冻结且非 NULL 的 `auctions.intake_opens_at`；没有冻结边界时不产生提醒，
    /// 也不从当前配置推测边界。
    fn season_reminder_facts(
        &self,
        now: DateTime<Utc>,
        lead_seconds: i64,
        limit: u32,
    ) -> ApplicationFuture<'_, Vec<SeasonReminderFact>>;
}
