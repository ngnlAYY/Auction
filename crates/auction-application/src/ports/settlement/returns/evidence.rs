//! 结算退件证据的消费判定。
//!
//! 邮件处理器只按系统来源把退件记为证据；存储层消费前再对持久化的发件人与主题复核来源。恢复
//! 流程只消费系统来源、绑定恢复记录付款或交付义务的退件；其余退件由
//! [`SettlementReturnFacts::classify`] 判定：先证明来源，再按正文派发引用唯一绑定原派发（首寄
//! 没有引用，补寄另须核对原正文摘要与原收件人），最后按履约状态决定是否转人工对账。本模块只做
//! 纯判定，不访问数据库、不写日志。

use auction_domain::{
    FIRST_DISPATCH_NO, SettlementDispatchReference, SettlementObligationKind, valid_text,
};
use auction_time::{TimeDelta, Timestamp};
use serde::Deserialize;
use uuid::Uuid;

use super::source::{system_return_matches_obligation, system_returned_settlement_obligation_id};

/// 退件收件时间下界在原投递开始发送所在分钟之前再留出的远端时钟余量（收件箱时间只到分钟）。
const RETURN_CLOCK_SLACK: TimeDelta = TimeDelta::minutes(2);

/// 单封退件证据允许记录的附件数上限，与恢复流程的证据校验一致。
const MAX_RETURN_ATTACHMENTS: usize = 100;

/// 邮件处理器写入的退件证据（`inbound_mail.decision_metadata`）；未知键、缺键或越界都视为损坏。
/// 正文三项只由能解析系统后缀的新处理器写入，更早的证据没有。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettlementReturnEvidence {
    obligation_id: String,
    credits: i64,
    cod: i64,
    /// 附件外部装备 ID，严格升序。
    attachment_ids: Vec<String>,
    /// 系统后缀写明的原收件人。
    #[serde(default)]
    returned_from: Option<String>,
    /// 原正文 canonical 摘要的小写十六进制。
    #[serde(default)]
    body_digest: Option<String>,
    /// 原正文末行派发引用原文；首寄退件没有。
    #[serde(default, deserialize_with = "deserialize_dispatch_reference")]
    dispatch_reference: Option<String>,
}

fn deserialize_dispatch_reference<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer).map(Some)
}

impl SettlementReturnEvidence {
    fn parse(metadata: &serde_json::Value) -> Option<Self> {
        Self::deserialize(metadata).ok()
    }

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
            })
            && self.returned_from.as_deref().is_none_or(|value| valid_text(value, 256))
            && self.body_digest.as_deref().is_none_or(|value| decode_digest(value).is_some());
        valid.then_some(obligation_id)
    }

    fn dispatch(&self) -> SettlementReturnDispatchRequest {
        match self.dispatch_reference.as_deref() {
            None => SettlementReturnDispatchRequest::First,
            Some(value) => SettlementDispatchReference::parse(value)
                .map_or(SettlementReturnDispatchRequest::Invalid, |reference| {
                    SettlementReturnDispatchRequest::Resend(reference)
                }),
        }
    }
}

/// 解析小写十六进制摘要；长度或字符不合法时返回 `None`。
fn decode_digest(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 || !value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let mut digest = [0_u8; 32];
    for (index, chunk) in value.as_bytes().chunks(2).enumerate() {
        let text = std::str::from_utf8(chunk).ok()?;
        digest[index] = u8::from_str_radix(text, 16).ok()?;
    }
    Some(digest)
}

/// 退件证据请求绑定的派发：没有引用即首寄；引用存在但不是规范编码时失败关闭。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementReturnDispatchRequest {
    First,
    Resend(SettlementDispatchReference),
    Invalid,
}

/// 读取证据请求的派发；证据无法解析时按首寄处理，由判定再以证据损坏失败关闭。
pub fn settlement_return_dispatch_request(
    metadata: &serde_json::Value,
) -> SettlementReturnDispatchRequest {
    SettlementReturnEvidence::parse(metadata)
        .map_or(SettlementReturnDispatchRequest::First, |evidence| evidence.dispatch())
}

/// 退件实际附带的资产（Take 的精确预期）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementReturnAssets {
    pub credits: i64,
    pub cod: i64,
    /// 附件外部装备 ID，严格升序。
    pub attachment_ids: Vec<String>,
}

/// 读取证据记录的退件资产；只接受完整且合法的证据，损坏时返回 `None`。
pub fn settlement_return_assets(metadata: &serde_json::Value) -> Option<SettlementReturnAssets> {
    let evidence = SettlementReturnEvidence::parse(metadata)?;
    evidence.obligation()?;
    Some(SettlementReturnAssets {
        credits: evidence.credits,
        cod: evidence.cod,
        attachment_ids: evidence.attachment_ids,
    })
}

/// 证据记录的系统后缀事实：原收件人与退件原正文的规范摘要。邮箱巡检据此核对 Take 之后的详情
/// 页正文仍以原退件正文开头；证据损坏或缺这两项（系统后缀不完整的退件）时返回 `None`。
pub fn settlement_return_original_body(metadata: &serde_json::Value) -> Option<(String, [u8; 32])> {
    let evidence = SettlementReturnEvidence::parse(metadata)?;
    evidence.obligation()?;
    let digest = decode_digest(evidence.body_digest.as_deref()?)?;
    Some((evidence.returned_from?, digest))
}

/// 恢复流程比对一封系统退件所需的证据投影。与结算退件消费方共用同一严格模型：未知键、缺键、
/// 越界或附件未排序都视为损坏；邮件处理器按系统后缀追加的正文事实（原收件人、原正文摘要）
/// 只校验形状，派发引用按规范编码解析。两个消费方各自维护一份字段集合时，处理器新增的证据键
/// 曾让恢复消费反序列化失败并停在队头。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryReturnEvidence {
    /// 证据记录的义务 ID（规范且非空）。
    pub obligation_id: Uuid,
    /// 退件实际附带的资产。
    pub assets: SettlementReturnAssets,
    /// 证据请求绑定的派发；恢复流程的付款与交付义务只有首寄，其余请求由消费方按证据不符隔离。
    pub dispatch: SettlementReturnDispatchRequest,
}

/// 读取恢复流程比对所需的证据；证据损坏时返回 `None`。
pub fn recovery_return_evidence(metadata: &serde_json::Value) -> Option<RecoveryReturnEvidence> {
    let evidence = SettlementReturnEvidence::parse(metadata)?;
    let obligation_id = evidence.obligation()?;
    let dispatch = evidence.dispatch();
    Some(RecoveryReturnEvidence {
        obligation_id,
        assets: SettlementReturnAssets {
            credits: evidence.credits,
            cod: evidence.cod,
            attachment_ids: evidence.attachment_ids,
        },
        dispatch,
    })
}

/// 存储层投影的退件消费事实；发件人与主题来自入站展示快照（更早的历史行为空）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementReturnFacts {
    pub sender: Option<String>,
    pub subject: Option<String>,
    pub return_world: String,
    pub received_at: Timestamp,
    /// `inbound_mail.decision_metadata` 原文。
    pub metadata: serde_json::Value,
    /// 证据义务 ID 对应的结算义务；不存在时为空。
    pub obligation: Option<SettlementReturnObligation>,
}

/// 已锁定的原结算义务、履约与按证据请求解析出的原派发。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementReturnObligation {
    pub id: Uuid,
    pub kind: String,
    pub amount: i64,
    pub carrier_credit: i64,
    pub external_equipment_id: Option<String>,
    /// 当前履约执行状态；尚未建立履约行时为空。
    pub execution_state: Option<String>,
    /// 按证据请求解析出的原派发投递；解析不唯一或不存在时为空。
    pub delivery: Option<SettlementReturnDelivery>,
    /// 派发解析结果。
    pub dispatch_lookup: SettlementReturnDispatchLookup,
}

/// 存储按证据请求解析原派发的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementReturnDispatchLookup {
    /// 唯一解析到一次派发。
    Unique,
    /// 请求的派发不存在（首寄尚未准备，或引用不属于任何已记录的补寄）。
    Missing,
    /// 首寄候选不唯一，不能默认绑定最新投递。
    Ambiguous,
    /// 该派发已被另一封退件绑定；一封邮件只会被退回一次。
    AlreadyReturned,
}

/// 原出站派发的发送事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementReturnDelivery {
    pub id: Uuid,
    /// 冻结载荷的世界（`payload_world`）。
    pub world: Option<String>,
    /// 冻结载荷的操作（`payload_operation`）。
    pub operation: Option<String>,
    /// 进入发送围栏的时刻；从未发送时为空。
    pub sending_at: Option<Timestamp>,
    /// 派发编号：首寄为 1。
    pub dispatch_no: u32,
    /// 是否为履约当前派发。
    pub current: bool,
    /// 冻结收件人。
    pub recipient: String,
    /// 该派发历次发送围栏冻结的正文摘要。
    pub body_digests: Vec<[u8; 32]>,
}

/// 结算退件消费结论闭集，即 `settlement_return_evidence.match_state`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementReturnMatch {
    /// 来源不是系统 `[RTS]` 退件（含已撤掉的无前缀兼容入口留下的历史记录）：只隔离，
    /// 不计入恢复退回次数，也不改变任何业务状态。
    Untrusted,
    /// 系统退件无法唯一绑定原派发：证据损坏、未知义务、主题与义务种类不符、派发未知或不唯一、
    /// 补寄正文无法证明、原派发未发送、世界不符或派发已被取代。只隔离。
    Unbound,
    /// 已绑定原派发，但履约已是终态（satisfied/failed/manual_reconcile）：只留证，不改写既有事实。
    FulfillmentTerminal,
    /// 已绑定原派发且履约未终结：同事务转人工对账。
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

    /// 解析持久化的处置名；未知值返回 `None`。
    pub fn from_name(value: &str) -> Option<Self> {
        [Self::Untrusted, Self::Unbound, Self::FulfillmentTerminal, Self::ManualReconcile]
            .into_iter()
            .find(|matched| matched.as_str() == value)
    }

    /// 是否已绑定原派发。
    pub const fn bound(self) -> bool {
        matches!(self, Self::FulfillmentTerminal | Self::ManualReconcile)
    }
}

/// 安全原因码闭集，即 `settlement_return_evidence.reason_code`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementReturnReason {
    /// 发件人或主题不能证明是系统退件。
    SourceUnproven,
    /// 证据对象损坏，或证据义务与主题义务不一致，或派发引用不是规范编码。
    EvidenceInvalid,
    /// 证据义务不存在。
    ObligationUnknown,
    /// 主题不是该义务种类的 canonical 主题。
    SubjectKindMismatch,
    /// 原履约没有已开始发送的结算 Send 投递。
    DeliveryUnsent,
    /// 退件所在世界与原投递世界不同。
    WorldMismatch,
    /// 派发引用不对应任何已记录的派发。
    DispatchUnknown,
    /// 没有派发引用而首寄候选不唯一。
    DispatchAmbiguous,
    /// 该派发已被另一封退件绑定。
    DispatchAlreadyReturned,
    /// 退件派发已不是履约当前派发。
    DispatchSuperseded,
    /// 补寄退件的原正文摘要或原收件人与派发冻结事实不符。
    BodyUnproven,
    /// 已绑定，退回资产与原投递冻结资产一致。
    ReturnedAssetsMatch,
    /// 已绑定，退回资产与原投递冻结资产不一致。
    ReturnedAssetsMismatch,
    /// 已绑定，但收件时间早于原投递开始发送（含时钟余量）。
    ReceivedBeforeSend,
}

impl SettlementReturnReason {
    /// 全部原因码，按声明顺序。
    pub const ALL: [Self; 14] = [
        Self::SourceUnproven,
        Self::EvidenceInvalid,
        Self::ObligationUnknown,
        Self::SubjectKindMismatch,
        Self::DeliveryUnsent,
        Self::WorldMismatch,
        Self::DispatchUnknown,
        Self::DispatchAmbiguous,
        Self::DispatchAlreadyReturned,
        Self::DispatchSuperseded,
        Self::BodyUnproven,
        Self::ReturnedAssetsMatch,
        Self::ReturnedAssetsMismatch,
        Self::ReceivedBeforeSend,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SourceUnproven => "source_unproven",
            Self::EvidenceInvalid => "evidence_invalid",
            Self::ObligationUnknown => "obligation_unknown",
            Self::SubjectKindMismatch => "subject_kind_mismatch",
            Self::DeliveryUnsent => "delivery_unsent",
            Self::WorldMismatch => "world_mismatch",
            Self::DispatchUnknown => "dispatch_unknown",
            Self::DispatchAmbiguous => "dispatch_ambiguous",
            Self::DispatchAlreadyReturned => "dispatch_already_returned",
            Self::DispatchSuperseded => "dispatch_superseded",
            Self::BodyUnproven => "body_unproven",
            Self::ReturnedAssetsMatch => "returned_assets_match",
            Self::ReturnedAssetsMismatch => "returned_assets_mismatch",
            Self::ReceivedBeforeSend => "received_before_send",
        }
    }

    /// 解析持久化的原因码；未知值返回 `None`。
    pub fn from_name(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|reason| reason.as_str() == value)
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
    /// 计算唯一结论：先证明来源，再绑定义务与原派发，最后按履约状态决定是否转人工。
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
        let Some(evidence) = SettlementReturnEvidence::parse(&self.metadata)
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
        let request = evidence.dispatch();
        if let Err(reason) = dispatch_binding(request, obligation) {
            return decided(M::Unbound, reason);
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
        if let SettlementReturnDispatchRequest::Resend(reference) = request
            && !resend_body_proven(&evidence, reference, delivery)
        {
            return decided(M::Unbound, R::BodyUnproven);
        }
        if delivery.world.as_deref() != Some(self.return_world.as_str()) {
            return decided(M::Unbound, R::WorldMismatch);
        }
        if !delivery.current {
            return decided(M::Unbound, R::DispatchSuperseded);
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

/// 派发请求与存储解析结果必须一致：首寄只认编号 1 的唯一派发；补寄引用必须属于该义务并解析到
/// 同一投递。首寄未准备时保留“未发送”原因，与引入派发编号前的判定一致。
fn dispatch_binding(
    request: SettlementReturnDispatchRequest,
    obligation: &SettlementReturnObligation,
) -> Result<(), SettlementReturnReason> {
    use SettlementReturnReason as R;

    match obligation.dispatch_lookup {
        SettlementReturnDispatchLookup::Ambiguous => return Err(R::DispatchAmbiguous),
        SettlementReturnDispatchLookup::AlreadyReturned => return Err(R::DispatchAlreadyReturned),
        SettlementReturnDispatchLookup::Missing => {
            return Err(match request {
                SettlementReturnDispatchRequest::First => R::DeliveryUnsent,
                SettlementReturnDispatchRequest::Resend(_) => R::DispatchUnknown,
                SettlementReturnDispatchRequest::Invalid => R::EvidenceInvalid,
            });
        }
        SettlementReturnDispatchLookup::Unique => {}
    }
    match request {
        SettlementReturnDispatchRequest::Invalid => Err(R::EvidenceInvalid),
        SettlementReturnDispatchRequest::First => match obligation.delivery.as_ref() {
            None => Err(R::DeliveryUnsent),
            Some(delivery) if delivery.dispatch_no == FIRST_DISPATCH_NO => Ok(()),
            Some(_) => Err(R::DispatchUnknown),
        },
        SettlementReturnDispatchRequest::Resend(reference) => {
            if !reference.belongs_to(obligation.id) {
                return Err(R::EvidenceInvalid);
            }
            match obligation.delivery.as_ref() {
                Some(delivery)
                    if delivery.id == reference.delivery_id
                        && delivery.dispatch_no == reference.dispatch_no =>
                {
                    Ok(())
                }
                _ => Err(R::DispatchUnknown),
            }
        }
    }
}

/// 补寄退件只凭正文绑定派发，因此必须同时证明原正文摘要等于该派发某次发送围栏冻结的摘要，
/// 且系统后缀写明的原收件人就是该派发的冻结收件人（ASCII 不区分大小写）。
fn resend_body_proven(
    evidence: &SettlementReturnEvidence,
    reference: SettlementDispatchReference,
    delivery: &SettlementReturnDelivery,
) -> bool {
    let digest = evidence.body_digest.as_deref().and_then(decode_digest);
    delivery.id == reference.delivery_id
        && digest.is_some_and(|digest| delivery.body_digests.contains(&digest))
        && evidence
            .returned_from
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case(&delivery.recipient))
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
    pub recorded_at: Timestamp,
}
