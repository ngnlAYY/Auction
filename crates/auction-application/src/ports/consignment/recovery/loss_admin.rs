//! Admin 损失案件的读取与人工结果写端口；读模型不含装备 key、URL、证据原文或任何金额字段。

use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::consignment::errors::ConsignmentFuture;
use crate::ports::consignment::recovery::{LossCaseOutcomeReceipt, RecordLossCaseOutcomeCommand};

/// 损失案件的状态；只读状态，不授予付款或发行能力（决定 22）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LossCaseState {
    /// 已开案，尚无人工结果。
    Opened,
    /// 复核中。
    Reviewing,
    /// 已办结。
    Closed,
}

impl LossCaseState {
    /// 稳定 wire 与存储码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Opened => "opened",
            Self::Reviewing => "reviewing",
            Self::Closed => "closed",
        }
    }

    /// 解析存储码；未知值返回 `None`，调用方失败关闭。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "opened" => Some(Self::Opened),
            "reviewing" => Some(Self::Reviewing),
            "closed" => Some(Self::Closed),
            _ => None,
        }
    }
}

/// 案件的一条追加记录（开案或人工结果）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminLossOutcomeRecord {
    /// 结果记录 UUID。
    pub outcome_id: Uuid,
    /// 该记录对应的案件版本。
    pub case_version: u64,
    /// 该记录的状态。
    pub state: LossCaseState,
    /// 依据 UUID；开案记录为空。
    pub basis_id: Option<Uuid>,
    /// 私有说明；开案记录为空。
    pub private_note: Option<String>,
    /// 记录时刻。
    pub occurred_at: Timestamp,
    /// 随该记录写下的卖家回执 UUID。
    pub seller_receipt_id: Uuid,
}

/// Admin 按 position 查到的损失案件；证据只给来源与摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminLossCaseRecord {
    /// position UUID。
    pub position_id: Uuid,
    /// 案件 UUID。
    pub case_id: Uuid,
    /// 唯一损失事实 UUID。
    pub loss_fact_id: Uuid,
    /// 支撑损失事实的证据 UUID。
    pub proof_id: Uuid,
    /// 证据来源。
    pub proof_source_kind: String,
    /// 证据摘要。
    pub proof_digest: [u8; 32],
    /// 挂单 UUID。
    pub listing_id: Uuid,
    /// 卖家本地用户 UUID。
    pub seller_user_id: Uuid,
    /// 确认灭失时的处分代。
    pub generation: u64,
    /// 最新案件版本。
    pub case_version: u64,
    /// 当前状态。
    pub state: LossCaseState,
    /// 按版本升序的全部记录。
    pub outcomes: Vec<AdminLossOutcomeRecord>,
}

/// 一条“无法查清”的核对记录：灭失确认因当前代权责或相反外写读不清而被拒，占用与双方责任保留。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LossReconcileRecord {
    /// 被拒的确认命令 UUID（核对记录主键）。
    pub command_id: Uuid,
    /// position UUID。
    pub position_id: Uuid,
    /// 记录时刻。
    pub recorded_at: Timestamp,
}

/// Admin 损失案件端口：读案件、列未结核对案与追加人工结果。人工结果回执不含金额、付款或发行字段。
pub trait LossCaseAdmin: Send + Sync {
    /// 按 position 读案件；没有案件返回 `None`。
    fn admin_loss_case(
        &self,
        position_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<AdminLossCaseRecord>>;

    /// 列出某 position 尚未结案的核对记录（该 position 还没有损失终局），按时间升序。
    fn open_loss_reconciles(
        &self,
        position_id: Uuid,
    ) -> ConsignmentFuture<'_, Vec<LossReconcileRecord>>;

    /// 追加人工结果；同命令 ID 同请求返回原回执，异请求为 `command_id_conflict`。
    fn record_loss_case_outcome(
        &self,
        command: RecordLossCaseOutcomeCommand,
    ) -> ConsignmentFuture<'_, LossCaseOutcomeReceipt>;
}
