//! 结算退件的来源认证、证据与消费判定。
//!
//! 收件人拒收（或过期）的结算邮件由系统发件人 `MoogleMail` 以 `[RTS] ` + 原 canonical 主题
//! 退回 Bot 收件箱。邮件处理器只按 [`system_returned_settlement_obligation_id`] 把这种退件记为
//! 证据；存储层消费前再对持久化的发件人与主题用同一规则复核来源。义务 ID 可由公开的
//! auction/lot ID 推出，不能作为来源证明。恢复流程只消费系统来源、绑定恢复记录付款或交付义务
//! 的退件；其余退件由 [`SettlementReturnFacts::classify`] 判定处置：只留证、对应未终结履约转
//! 人工对账，不领取、不退回、不重发，也不改变资金。本模块只做纯判定，不访问数据库、不写日志。

use auction_domain::{SettlementId, SettlementObligationKind, valid_text};
use chrono::{DateTime, TimeDelta, Timelike, Utc};
use serde::Deserialize;
use uuid::Uuid;

use super::fulfillment::canonical_settlement_fulfillment_subject;

/// MoogleMail 系统退信的发件人；精确比较、不忽略大小写，玩家不能使用这个名字发信。
pub const SYSTEM_RETURN_SENDER: &str = "MoogleMail";

/// 系统退信加在原主题前的前缀（ASCII 小写形式）。
const RETURNED_SUBJECT_PREFIX: &str = "[rts] ";

/// 结算邮件 canonical 主题的基名（ASCII 小写），与 `canonical_settlement_fulfillment_subject`
/// 的四种主题一一对应。
const SETTLEMENT_SUBJECT_BASES: [&str; 4] = [
    "auction equipment delivery",
    "auction payment request",
    "auction seller payout",
    "auction buyer refund",
];

/// 退件收件时间下界在原投递开始发送所在分钟之前再留出的远端时钟余量（收件箱时间只到分钟）。
const RETURN_CLOCK_SLACK: TimeDelta = TimeDelta::minutes(2);

/// 单封退件证据允许记录的附件数上限，与恢复流程的证据校验一致。
const MAX_RETURN_ATTACHMENTS: usize = 100;

/// 发件人恰为系统且主题为 `[RTS] ` + canonical 结算主题时，返回主题里的义务 ID。主题按 ASCII
/// 不区分大小写比较，UUID 必须是规范小写形式；无前缀主题、玩家自拟的同名主题与其他系统退信
/// 都返回 `None`。
pub fn system_returned_settlement_obligation_id(sender: &str, subject: &str) -> Option<Uuid> {
    if sender != SYSTEM_RETURN_SENDER {
        return None;
    }
    let subject = subject.to_ascii_lowercase();
    let (base, encoded) = subject.strip_prefix(RETURNED_SUBJECT_PREFIX)?.rsplit_once(" [")?;
    let encoded = encoded.strip_suffix(']')?;
    if !SETTLEMENT_SUBJECT_BASES.contains(&base) {
        return None;
    }
    let obligation_id = Uuid::parse_str(encoded).ok()?;
    (!obligation_id.is_nil() && obligation_id.to_string() == encoded).then_some(obligation_id)
}

/// 系统退件的主题是否恰为该义务种类的 canonical 主题加 `[RTS] ` 前缀（ASCII 不区分大小写）；
/// 比 [`system_returned_settlement_obligation_id`] 更严格，同时绑定义务种类。
pub fn system_return_matches_obligation(
    sender: &str,
    subject: &str,
    kind: SettlementObligationKind,
    obligation_id: Uuid,
) -> bool {
    sender == SYSTEM_RETURN_SENDER
        && canonical_settlement_fulfillment_subject(kind, SettlementId::from_uuid(obligation_id))
            .is_some_and(|canonical| subject.eq_ignore_ascii_case(&format!("[RTS] {canonical}")))
}

/// 邮件处理器写入的退件证据（`inbound_mail.decision_metadata`）；未知键、缺键或越界都视为损坏。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettlementReturnEvidence {
    obligation_id: String,
    credits: i64,
    cod: i64,
    /// 附件外部装备 ID，严格升序。
    attachment_ids: Vec<String>,
}

impl SettlementReturnEvidence {
    /// 规范且非空的义务 ID；证据越界或附件未排序时返回 `None`。
    fn obligation(&self) -> Option<Uuid> {
        let obligation_id = Uuid::parse_str(&self.obligation_id).ok()?;
        let valid = !obligation_id.is_nil()
            && obligation_id.to_string() == self.obligation_id
            && self.credits >= 0
            && self.cod >= 0
            && self.attachment_ids.len() <= MAX_RETURN_ATTACHMENTS
            && self.attachment_ids.iter().enumerate().all(|(index, value)| {
                valid_text(value, 128)
                    && (index == 0 || self.attachment_ids[index - 1].as_str() < value.as_str())
            });
        valid.then_some(obligation_id)
    }
}

/// 存储层投影的退件消费事实；发件人与主题来自入站展示快照（更早的历史行为空）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementReturnFacts {
    pub sender: Option<String>,
    pub subject: Option<String>,
    pub return_world: String,
    pub received_at: DateTime<Utc>,
    /// `inbound_mail.decision_metadata` 原文。
    pub metadata: serde_json::Value,
    /// 证据义务 ID 对应的结算义务；不存在时为空。
    pub obligation: Option<SettlementReturnObligation>,
}

/// 已锁定的原结算义务、履约与原出站投递。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementReturnObligation {
    pub id: Uuid,
    pub kind: String,
    pub amount: i64,
    pub carrier_credit: i64,
    pub external_equipment_id: Option<String>,
    /// 当前履约执行状态；尚未建立履约行时为空。
    pub execution_state: Option<String>,
    /// 履约绑定、来源为该义务的结算投递；未准备时为空。
    pub delivery: Option<SettlementReturnDelivery>,
}

/// 原出站投递的发送事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementReturnDelivery {
    pub id: Uuid,
    /// 冻结载荷的世界（`payload_world`）。
    pub world: Option<String>,
    /// 冻结载荷的操作（`payload_operation`）。
    pub operation: Option<String>,
    /// 进入发送围栏的时刻；从未发送时为空。
    pub sending_at: Option<DateTime<Utc>>,
}

/// 结算退件消费结论闭集，即 `settlement_return_evidence.match_state`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementReturnMatch {
    /// 来源不是系统 `[RTS]` 退件（含已撤掉的无前缀兼容入口留下的历史记录）：只隔离，
    /// 不计入恢复退回次数，也不改变任何业务状态。
    Untrusted,
    /// 系统退件无法唯一绑定原出站投递：证据损坏、未知义务、主题与义务种类不符、原投递未发送
    /// 或世界不符。只隔离。
    Unbound,
    /// 已绑定原投递，但履约已是终态（satisfied/failed/manual_reconcile）：只留证，不改写既有事实。
    FulfillmentTerminal,
    /// 已绑定原投递且履约未终结：同事务转人工对账。
    ManualReconcile,
}

impl SettlementReturnMatch {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Untrusted => "untrusted",
            Self::Unbound => "unbound",
            Self::FulfillmentTerminal => "fulfillment_terminal",
            Self::ManualReconcile => "manual_reconcile",
        }
    }
}

/// 安全原因码闭集，即 `settlement_return_evidence.reason_code`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementReturnReason {
    /// 发件人或主题不能证明是系统退件。
    SourceUnproven,
    /// 证据对象损坏，或证据义务与主题义务不一致。
    EvidenceInvalid,
    /// 证据义务不存在。
    ObligationUnknown,
    /// 主题不是该义务种类的 canonical 主题。
    SubjectKindMismatch,
    /// 原履约没有已开始发送的结算 Send 投递。
    DeliveryUnsent,
    /// 退件所在世界与原投递世界不同。
    WorldMismatch,
    /// 已绑定，退回资产与原投递冻结资产一致。
    ReturnedAssetsMatch,
    /// 已绑定，退回资产与原投递冻结资产不一致。
    ReturnedAssetsMismatch,
    /// 已绑定，但收件时间早于原投递开始发送（含时钟余量）。
    ReceivedBeforeSend,
}

impl SettlementReturnReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SourceUnproven => "source_unproven",
            Self::EvidenceInvalid => "evidence_invalid",
            Self::ObligationUnknown => "obligation_unknown",
            Self::SubjectKindMismatch => "subject_kind_mismatch",
            Self::DeliveryUnsent => "delivery_unsent",
            Self::WorldMismatch => "world_mismatch",
            Self::ReturnedAssetsMatch => "returned_assets_match",
            Self::ReturnedAssetsMismatch => "returned_assets_mismatch",
            Self::ReceivedBeforeSend => "received_before_send",
        }
    }
}

/// 一封退件的处置结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettlementReturnDecision {
    pub matched: SettlementReturnMatch,
    pub reason: SettlementReturnReason,
}

const fn decided(
    matched: SettlementReturnMatch,
    reason: SettlementReturnReason,
) -> SettlementReturnDecision {
    SettlementReturnDecision { matched, reason }
}

impl SettlementReturnFacts {
    /// 计算唯一结论：先证明来源，再绑定义务与原投递，最后按履约状态决定是否转人工。
    pub fn classify(&self) -> SettlementReturnDecision {
        use SettlementReturnMatch as M;
        use SettlementReturnReason as R;

        let (Some(sender), Some(subject)) = (self.sender.as_deref(), self.subject.as_deref())
        else {
            return decided(M::Untrusted, R::SourceUnproven);
        };
        let Some(source_obligation) = system_returned_settlement_obligation_id(sender, subject)
        else {
            return decided(M::Untrusted, R::SourceUnproven);
        };
        let Some(evidence) = SettlementReturnEvidence::deserialize(&self.metadata)
            .ok()
            .filter(|evidence| evidence.obligation() == Some(source_obligation))
        else {
            return decided(M::Unbound, R::EvidenceInvalid);
        };
        let Some(obligation) =
            self.obligation.as_ref().filter(|obligation| obligation.id == source_obligation)
        else {
            return decided(M::Unbound, R::ObligationUnknown);
        };
        let Some(kind) = SettlementObligationKind::from_name(&obligation.kind) else {
            return decided(M::Unbound, R::ObligationUnknown);
        };
        if !system_return_matches_obligation(sender, subject, kind, obligation.id) {
            return decided(M::Unbound, R::SubjectKindMismatch);
        }
        let (Some(state), Some(delivery)) =
            (obligation.execution_state.as_deref(), obligation.delivery.as_ref())
        else {
            return decided(M::Unbound, R::DeliveryUnsent);
        };
        let Some(sending_at) =
            delivery.sending_at.filter(|_| delivery.operation.as_deref() == Some("send"))
        else {
            return decided(M::Unbound, R::DeliveryUnsent);
        };
        if delivery.world.as_deref() != Some(self.return_world.as_str()) {
            return decided(M::Unbound, R::WorldMismatch);
        }
        let matched = match state {
            "pending" | "prepared" | "enqueued" => M::ManualReconcile,
            "satisfied" | "failed" | "manual_reconcile" => M::FulfillmentTerminal,
            _ => return decided(M::Unbound, R::DeliveryUnsent),
        };
        let earliest = sending_at
            .with_second(0)
            .and_then(|value| value.with_nanosecond(0))
            .and_then(|value| value.checked_sub_signed(RETURN_CLOCK_SLACK));
        let reason = if earliest.is_none_or(|earliest| self.received_at < earliest) {
            R::ReceivedBeforeSend
        } else if expected_assets(kind, obligation).is_some_and(|(credits, cod, attachments)| {
            evidence.credits == credits
                && evidence.cod == cod
                && evidence.attachment_ids == attachments
        }) {
            R::ReturnedAssetsMatch
        } else {
            R::ReturnedAssetsMismatch
        };
        decided(matched, reason)
    }
}

/// 原投递冻结资产被原样退回时的证据：现金类退回原 Credits，装备类退回原装备；退件不保留 COD。
fn expected_assets(
    kind: SettlementObligationKind,
    obligation: &SettlementReturnObligation,
) -> Option<(i64, i64, Vec<String>)> {
    match kind {
        SettlementObligationKind::SellerPayout | SettlementObligationKind::BuyerRefund => {
            Some((obligation.amount, 0, Vec::new()))
        }
        SettlementObligationKind::WinnerPaymentCarrierCod => {
            Some((obligation.carrier_credit, 0, Vec::new()))
        }
        SettlementObligationKind::WinnerEquipmentCod
        | SettlementObligationKind::WinnerEquipmentHandoff
        | SettlementObligationKind::UnsoldReturn
        | SettlementObligationKind::WithdrawnReturn => obligation
            .external_equipment_id
            .as_ref()
            .filter(|id| valid_text(id, 128))
            .map(|id| (0, 0, vec![id.clone()])),
    }
}

/// Worker 消费一封结算退件后返回的安全摘要；不含邮件正文、发件人或主题。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementReturnConsumption {
    pub return_mail_id: Uuid,
    pub obligation_id: Option<Uuid>,
    pub delivery_id: Option<Uuid>,
    pub decision: SettlementReturnDecision,
    pub recorded_at: DateTime<Utc>,
}
