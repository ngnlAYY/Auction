//! 原信观察、预约映射、整封决定与退回清单的类型闭集。

use auction_time::Timestamp;

use crate::UserId;
use crate::consignment::ListingEquipmentIdentity;
use crate::consignment::listing::ListingVersion;
use crate::delivery::OriginalMailKey;

/// 旧拍卖 owner；只有既有拍卖路由无法归属时才由寄售隔离交接。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LegacyAuctionOwner {
    /// 旧拍卖卖款退回。
    SellerPayout,
    /// 旧拍卖买家退款。
    BuyerRefund,
    /// 旧拍卖流拍返还。
    UnsoldReturn,
    /// 旧拍卖撤拍返还。
    WithdrawnReturn,
    /// main 旧恢复记录的装备 COD 付款义务。
    WinnerEquipmentCod,
    /// isekai 在 main 寄出的付款载体 COD 义务。
    WinnerPaymentCarrierCod,
    /// isekai 原世界的装备交割义务。
    WinnerEquipmentHandoff,
}

impl LegacyAuctionOwner {
    /// 稳定 wire 与审计码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SellerPayout => "seller_payout",
            Self::BuyerRefund => "buyer_refund",
            Self::UnsoldReturn => "unsold_return",
            Self::WithdrawnReturn => "withdrawn_return",
            Self::WinnerEquipmentCod => "winner_equipment_cod",
            Self::WinnerPaymentCarrierCod => "winner_payment_carrier_cod",
            Self::WinnerEquipmentHandoff => "winner_equipment_handoff",
        }
    }

    /// 闭集解析，未知值失败关闭。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "seller_payout" => Some(Self::SellerPayout),
            "buyer_refund" => Some(Self::BuyerRefund),
            "unsold_return" => Some(Self::UnsoldReturn),
            "withdrawn_return" => Some(Self::WithdrawnReturn),
            "winner_equipment_cod" => Some(Self::WinnerEquipmentCod),
            "winner_payment_carrier_cod" => Some(Self::WinnerPaymentCarrierCod),
            "winner_equipment_handoff" => Some(Self::WinnerEquipmentHandoff),
            _ => None,
        }
    }

    /// 是否交还原拍卖退件案件 owner。
    pub const fn is_return_case(self) -> bool {
        matches!(
            self,
            Self::SellerPayout | Self::BuyerRefund | Self::UnsoldReturn | Self::WithdrawnReturn
        )
    }

    /// 是否交还原拍卖恢复 owner。
    pub const fn is_recovery(self) -> bool {
        matches!(
            self,
            Self::WinnerEquipmentCod | Self::WinnerPaymentCarrierCod | Self::WinnerEquipmentHandoff
        )
    }
}

/// 隔离原因闭集；只携带安全分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuarantineReason {
    /// 邮件包含重复原文 EID，解析失败。
    DuplicateRawEid,
    /// `42` 与 `00042` 等同世界别名无法唯一匹配。
    CanonicalAliasAmbiguous,
    /// 附件布局或种类未知。
    UnknownAttachment,
    /// 列表与详情清单冲突。
    ListDetailConflict,
    /// 资产数量无法确定。
    AssetCountUnknown,
    /// 同名显示身份换了认证 UID。
    SameNameChangedUid,
    /// 发件人或认证身份无法证明。
    IdentityUnproven,
    /// 金额/COD 形状无法证明。
    AmountUnproven,
    /// 无法证明可安全原样退回。
    ReturnUnproven,
    /// 取得能力或该封的 `takeable` 观察未证明。
    ///
    /// 整封隔离，零预约消费，也不产生 Take 工作。
    TakeUnproven,
    /// 邮件形状不能安全原样退回或会扣款。
    UnsafeReturnShape,
    /// 疑似系统 RTS，但既有拍卖路由无法归属。
    SuspectedSystemRts,
}

impl QuarantineReason {
    /// 稳定 wire 与审计码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DuplicateRawEid => "duplicate_raw_eid",
            Self::CanonicalAliasAmbiguous => "canonical_alias_ambiguous",
            Self::UnknownAttachment => "unknown_attachment",
            Self::ListDetailConflict => "list_detail_conflict",
            Self::AssetCountUnknown => "asset_count_unknown",
            Self::SameNameChangedUid => "same_name_changed_uid",
            Self::IdentityUnproven => "identity_unproven",
            Self::AmountUnproven => "amount_unproven",
            Self::ReturnUnproven => "return_unproven",
            Self::TakeUnproven => "take_unproven",
            Self::UnsafeReturnShape => "unsafe_return_shape",
            Self::SuspectedSystemRts => "suspected_system_rts",
        }
    }
}

/// 暂不决定、保留原定位的安全原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PendingReason {
    /// 临时限流、战斗或读取失败。
    TransientReadFailure,
    /// 认证 UID 尚不能建立。
    MissingAuthenticatedUid,
    /// 已有最终决定，调用方须恢复原动作。
    ExistingDecision,
    /// 主题不是 `consign`，不属于寄售分类。
    NotConsignSubject,
}

impl PendingReason {
    /// 稳定 wire 与审计码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TransientReadFailure => "transient_read_failure",
            Self::MissingAuthenticatedUid => "missing_authenticated_uid",
            Self::ExistingDecision => "existing_decision",
            Self::NotConsignSubject => "not_consign_subject",
        }
    }
}

/// 发件人显示身份与认证 UID 的关系。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SenderIdentity {
    /// 已认证且显示名与认证身份一致，改名不改变 UID。
    Verified,
    /// 同名但认证 Bot UID 已变化。
    SameNameDifferentUid,
    /// 无法证明发件人身份。
    Unverified,
}

/// 原信附件种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnvelopeItemKind {
    /// 可识别装备。
    Equipment,
    /// Credits 附件。
    Credits,
    /// COD 附件。
    Cod,
    /// 未知附件。
    Unknown,
}

impl EnvelopeItemKind {
    /// 稳定 wire 码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Equipment => "equipment",
            Self::Credits => "credits",
            Self::Cod => "cod",
            Self::Unknown => "unknown",
        }
    }
}

/// 原信中的一件附件观察，保留原始序号。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvelopeItemObservation {
    /// 原始附件序号，从 1 开始。
    pub ordinal: u32,
    /// 附件种类。
    pub kind: EnvelopeItemKind,
    /// 页面显示名；用于系统领取 marker 的逐项配对。
    pub display_name: Option<String>,
    /// 原文 EID；无法读取时为空。
    pub raw_eid: Option<String>,
    /// 规范正十进制 EID；未知或歧义时为空。
    pub canonical_eid: Option<String>,
    /// Credits/COD 附件的金额；装备附件为空。
    pub amount: Option<u64>,
    /// Bot 在原世界读到的来源世界。
    pub world: Option<crate::SettlementWorld>,
    /// 发件人对应的本站卖家；无法唯一解析时为空。
    pub seller_user_id: Option<UserId>,
    /// Bot 在原世界确认可交易。
    pub tradeable: bool,
    /// 历史门通过。
    pub history_ok: bool,
    /// 受保护凭证引用。
    pub protected_ref: Option<uuid::Uuid>,
}

/// 预约在锁内的有效状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReservationState {
    /// 当前有效。
    Active,
    /// 已换发或取消。
    Retired,
    /// 已被一次接受消费。
    Consumed,
}

impl ReservationState {
    /// 稳定 wire 码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Retired => "retired",
            Self::Consumed => "consumed",
        }
    }
}

/// 锁内预约及其冻结身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationBinding {
    /// 预约 UUID。
    pub reservation_id: uuid::Uuid,
    /// 预约代。
    pub generation: u64,
    /// 目标挂单。
    pub listing_id: uuid::Uuid,
    /// 签发时冻结的挂单版本。
    pub listing_version: ListingVersion,
    /// 卖家。
    pub seller_user_id: UserId,
    /// 世界与规范 EID。
    pub equipment: ListingEquipmentIdentity,
    /// 预约截止。
    pub expires_at: Timestamp,
    /// 锁内状态。
    pub state: ReservationState,
}

/// 系统 RTS 上可复用的旧退件证据；取值口径与旧 `inbound_mail.decision_metadata` 相同。
///
/// 这些键只保存系统主题与正文后缀已经证明的事实；任一键读不出时为空，不猜测旧案件。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyReturnEvidence {
    /// 规范结算主题里的义务 UUID；非规范结算退件为空。
    pub obligation_id: Option<uuid::Uuid>,
    /// 完整系统主题；只在规范结算退件上保留，用于补写旧格式入站行。
    pub subject: Option<String>,
    /// 系统后缀写明的原收件人。
    pub returned_from: Option<String>,
    /// 原正文 canonical 摘要。
    pub body_digest: Option<[u8; 32]>,
    /// 原正文末行的派发引用原文。
    pub dispatch_reference: Option<String>,
}

/// 一次原信观察的完整安全事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvelopeObservation {
    /// 本次原观察 UUID。
    pub observation_id: uuid::Uuid,
    /// 证据会话绑定摘要。
    pub session_binding_digest: [u8; 32],
    /// 冻结清单摘要。
    pub manifest_digest: [u8; 32],
    /// 解析器版本。
    pub parser_version: String,
    /// 主题是否为 `consign`。
    pub subject_is_consign: bool,
    /// 发件人身份关系。
    pub sender_identity: SenderIdentity,
    /// 列表与详情清单一致。
    pub list_detail_consistent: bool,
    /// 原始附件数；读不出时为 `None`。
    pub asset_count: Option<u32>,
    /// 重复原文 EID。
    pub duplicate_raw_eid: bool,
    /// 规范别名歧义。
    pub canonical_alias_ambiguous: bool,
    /// 存在未知附件。
    pub unknown_attachment: bool,
    /// Credits 金额；读不出时为 `None`。
    pub credits: Option<i64>,
    /// COD 金额；读不出时为 `None`。
    pub cod: Option<i64>,
    /// 服务器允许原样 Return。
    pub returnable: bool,
    /// 服务器允许该封 Take；读不出或无法证明时为空。
    pub takeable: Option<bool>,
    /// 疑似系统 RTS。
    pub system_rts: bool,
    /// 已识别的旧拍卖 owner；存在时先走原 owner。
    pub legacy_auction_owner: Option<LegacyAuctionOwner>,
    /// 原始附件观察，保留原序。
    pub items: Vec<EnvelopeItemObservation>,
}

/// 与具体邮件无关的收件能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvelopeCapabilities {
    /// 已签收的形状证明“原样退回且不扣款”。
    pub safe_original_return: bool,
    /// 该世界已签收的取得验证器；第 4 期未签收时为 false。
    ///
    /// 装配层按世界给出，领域只读结果。为 false 时任何整封都不得进入 Accept。
    pub take_ready: bool,
}

/// `decide_envelope` 的全部锁内输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedEnvelopeFacts {
    /// 稳定原信键；认证 UID 尚不能建立时为空。
    pub mail_key: Option<OriginalMailKey>,
    /// 一次原观察。
    pub observation: EnvelopeObservation,
    /// 涉及的当前预约。
    pub reservations: Vec<ReservationBinding>,
    /// 发件人对应的唯一卖家。
    pub envelope_seller_user_id: Option<UserId>,
    /// 收件能力。
    pub capabilities: EnvelopeCapabilities,
    /// 临时读取失败。
    pub read_failure: bool,
    /// 是否已有最终决定。
    pub already_decided: bool,
    /// 锁后数据库时刻。
    pub now: Timestamp,
}

/// 一一对应且可接受的整封中的一件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedDepositItem {
    /// 原始附件序号。
    pub ordinal: u32,
    /// 目标挂单。
    pub listing_id: uuid::Uuid,
    /// 冻结挂单版本。
    pub listing_version: ListingVersion,
    /// 卖家。
    pub seller_user_id: UserId,
    /// 预约 UUID。
    pub reservation_id: uuid::Uuid,
    /// 预约代。
    pub reservation_generation: u64,
    /// 世界与规范 EID。
    pub equipment: ListingEquipmentIdentity,
    /// 受保护凭证引用。
    pub credential_ref: Option<uuid::Uuid>,
}

/// 整封退回中一件原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReturnItemReason {
    /// 未登记。
    Unregistered,
    /// 错人。
    WrongRecipient,
    /// 预约过期。
    ReservationExpired,
    /// 预约已撤销或消费。
    ReservationNotActive,
    /// 引用数不符。
    ReservationCountMismatch,
    /// 确定不可交易。
    NotTradeable,
    /// 历史不足。
    HistoryUnproven,
    /// 含 Credits 或 COD。
    AmountsPresent,
}

impl ReturnItemReason {
    /// 稳定 wire 码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unregistered => "unregistered",
            Self::WrongRecipient => "wrong_recipient",
            Self::ReservationExpired => "reservation_expired",
            Self::ReservationNotActive => "reservation_not_active",
            Self::ReservationCountMismatch => "reservation_count_mismatch",
            Self::NotTradeable => "not_tradeable",
            Self::HistoryUnproven => "history_unproven",
            Self::AmountsPresent => "amounts_present",
        }
    }
}

/// 原拒件清单中的一件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnItem {
    /// 原始附件序号。
    pub ordinal: u32,
    /// 规范 EID；无法读取时为空。
    pub canonical_eid: Option<String>,
    /// 拒收原因。
    pub reason: ReturnItemReason,
}

/// 原样退回的整封清单。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnManifest {
    /// 按原序号排列的拒件。
    pub items: Vec<ReturnItem>,
}

/// 已识别寄售来源的可信系统 RTS：只携带来源标识，原发送的其余事实由接管事务锁内重读。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedConsignmentRts {
    /// RTS 主题与后缀证明的原寄售义务。
    pub obligation_id: uuid::Uuid,
    /// 原发送轮。
    pub original_round: u32,
}

/// 整封决定的闭集。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvelopeDecision {
    /// 全封一一对应接受。
    Accept(Vec<MatchedDepositItem>),
    /// 整封原样退回，零预约消费。
    Return(ReturnManifest),
    /// 可认证且确定没有资产。
    ArchiveEmpty,
    /// 持久隔离，零资产 POST。
    Quarantine(QuarantineReason),
    /// 已识别寄售来源的可信 RTS：交退件接管写端，按 RTS 的 K 建 intake 与 Take 工作。
    AcquireReturn(TrustedConsignmentRts),
}

impl EnvelopeDecision {
    /// 进入 v2 摘要的稳定决定种类；证据内容另按种类编码。
    pub const fn decision_kind(&self) -> &'static str {
        match self {
            Self::Accept(_) => "accept",
            Self::Return(_) => "return",
            Self::ArchiveEmpty => "archive_empty",
            Self::Quarantine(_) => "quarantine",
            Self::AcquireReturn(_) => "acquire_return",
        }
    }
}

/// 原信的唯一路由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvelopeRoute {
    /// 交还既有拍卖 owner，零寄售资产动作。
    LegacyAuction(LegacyAuctionOwner),
    /// 保留原定位，稍后重读。
    Pending(PendingReason),
    /// 寄售自己的决定。
    Consignment(EnvelopeDecision),
}
