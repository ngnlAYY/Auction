//! 可信退回事实的输入形状：只含资格判定需要的状态与时刻，金额与库存不参与。

use auction_time::Timestamp;
use uuid::Uuid;

use crate::UserId;
use crate::delivery::OriginalMailKey;

/// 拒收种类，决定适用的处罚规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalKind {
    /// main 未付原件 COD 退回，按同一合同计数。
    MainCodReturn,
    /// isekai 未付载体退回，按买家跨合同、90 天窗口计数。
    IsekaiUnpaidCarrierReturn,
}

/// 可信 RTS 发生时原合同是否仍可交付（决定 26）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deliverability {
    /// 有效交割关闭界点之前，原合同仍可交付。
    Deliverable,
    /// 交割已关闭。
    Closed,
    /// 可交付性有实质争议，证清前不得处罚。
    Disputed,
}

/// 一条已保存的可信退回事实及资格判定所需的证据结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnFactInput {
    /// 退回事实 ID（见 [`crate::consignment::refusal::return_fact_id`]）。
    pub return_fact_id: Uuid,
    /// 原信键。
    pub k: OriginalMailKey,
    /// 寄售合同。
    pub contract_id: Uuid,
    /// 被退回的付款义务。
    pub obligation_id: Uuid,
    /// 原轮次。
    pub original_round: u32,
    /// 拒收种类。
    pub kind: RefusalKind,
    /// 买家本地账号。
    pub buyer_user_id: UserId,
    /// 可信远端 UTC 分钟起点，保留精度。
    pub returned_at: Timestamp,
    /// `returned_at` 加 1 分钟，用于跨界判断。
    pub returned_minute_end: Timestamp,
    /// 实际补证时刻；不参与资格先后。
    pub proven_at: Timestamp,
    /// isekai 有效交割关闭界点；main 为 `None`。
    pub delivery_close_at: Option<Timestamp>,
    /// 事实所属的拒收周期。
    pub cycle_id: Uuid,
    /// 上游给出的可交付性结论；仅对 isekai 起作用。
    pub deliverability: Deliverability,
    /// 同一合同已有相反的付款或实物交付证据（已付后的退回不计）。
    pub paid_or_opposite: bool,
    /// 退回物与原轮冻结的资产清单一致（载体 1 Credit 或原件）。
    pub asset_matches: bool,
    /// 同一 K、同一原轮已有事实（拍卖 / 寄售两个消费者重见也算重复）。
    pub duplicate: bool,
    /// 买家归属已被证明。
    pub ownership_proven: bool,
}
