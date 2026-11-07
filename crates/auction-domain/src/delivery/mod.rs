//! 投递终态处置事实的纯领域模型（W2）。
//!
//! 处置事实只描述“非正常成功路径的终态如何收口”：自动 Sent 恢复、人工 Sent 证明、
//! Admin 判失败、装备损失退役、历史自寄核对、发送围栏前的预检退役，以及操作者确认
//! 无资金与资产影响的历史失败已处置。普通一次成功发送不写处置行，因此该闭集只覆盖
//! 异常或不确定终态，不与正常成功路径重复计数。
//!
//! 每类准入事实只有一个来源引用：`DeliveryDispositionSource` 用“恰好一个非空引用”
//! 表达这一点；`PreflightRetired` 以投递行自身（投递 id、fence 与失败码）为证，不带
//! 外部引用。是否解除失败计数在写入时由 [`DeliveryDispositionKind::resolves_alert`]
//! 固定为事实列，读侧只按该列过滤，不再维护按来源拆分的排除谓词。

use uuid::Uuid;

use crate::text::{valid_positive_decimal, valid_text};

/// 预检退役中唯一解除失败计数的失败码；其余预检退役仍计入通知失败。
pub const PREFLIGHT_LOT_WATCH_INVALIDATED: &str = "lot_watch_invalidated";

/// 预检退役允许的失败码闭集；与发送围栏前的退役写入共用。
pub const PREFLIGHT_RETIREMENT_CODES: [&str; 4] = [
    PREFLIGHT_LOT_WATCH_INVALIDATED,
    "recipient_renamed",
    "auction_notice_invalidated",
    "season_notice_invalidated",
];

/// 处置种类闭集：只覆盖异常或不确定终态的收口，不覆盖每次普通成功发送。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeliveryDispositionKind {
    /// 维护扫描在完整 Sent 中自动恢复出绑定，把结果未知的发送改为成功。
    AutoSentProof,
    /// 正式运维命令的人工 Sent 观察（`delivery_sent_confirmations`）。
    OperatorSentConfirmation,
    /// Admin 对账判失败的命令结论。
    AdminFailed,
    /// 装备损失把原投递永久退役。
    EquipmentLoss,
    /// COD 第三次退回后成交撤销，原 COD 与卖家收款义务终结。
    CodReturned,
    /// 历史自寄结算核对认定原投递无需再发送（只追加修复表）。
    LegacySelf,
    /// 发送围栏前失去资格而退役；从未开始外部写入。
    PreflightRetired,
    /// 操作者确认无资金与资产影响的历史失败已经处置；必须填写原因。
    OperatorAcknowledged,
}

impl DeliveryDispositionKind {
    /// 全部种类；持久化闭集、应用层与迁移必须逐项对应。
    pub const ALL: [Self; 8] = [
        Self::AutoSentProof,
        Self::OperatorSentConfirmation,
        Self::AdminFailed,
        Self::EquipmentLoss,
        Self::CodReturned,
        Self::LegacySelf,
        Self::PreflightRetired,
        Self::OperatorAcknowledged,
    ];

    /// 持久化与协议使用的稳定名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AutoSentProof => "auto_sent_proof",
            Self::OperatorSentConfirmation => "operator_sent_confirmation",
            Self::AdminFailed => "admin_failed",
            Self::EquipmentLoss => "equipment_loss",
            Self::CodReturned => "cod_returned",
            Self::LegacySelf => "legacy_self",
            Self::PreflightRetired => "preflight_retired",
            Self::OperatorAcknowledged => "operator_acknowledged",
        }
    }

    /// 只接受闭集内的稳定名称；未知取值失败关闭。
    pub fn parse(value: &str) -> Result<Self, DeliveryDispositionError> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == value)
            .ok_or(DeliveryDispositionError::UnknownKind)
    }

    /// 操作者确认必须带有可追溯原因；其余自动或命令型处置的原因可选。
    pub const fn requires_operator_reason(self) -> bool {
        matches!(self, Self::OperatorAcknowledged)
    }

    /// 该处置是否解除投递与通知的失败计数。
    ///
    /// 当前口径沿用迁移 `000088` 之前的读侧排除：只有失去资格的关注提醒预检退役不计
    /// 失败，改名与赛季退役仍计入。放开其余退役属有意改变判定，须用户批准（决策 72）；
    /// 结论在写入时固定到 `delivery_dispositions.resolves_alert`，读侧只按该列过滤。
    pub const fn resolves_alert(self, reason_code: Option<&str>) -> bool {
        match self {
            Self::PreflightRetired => match reason_code {
                Some(code) => is_same(code, PREFLIGHT_LOT_WATCH_INVALIDATED),
                None => false,
            },
            Self::AutoSentProof
            | Self::OperatorSentConfirmation
            | Self::AdminFailed
            | Self::EquipmentLoss
            | Self::CodReturned
            | Self::LegacySelf
            | Self::OperatorAcknowledged => true,
        }
    }

    /// 该处置是否只由操作者命令写入（幂等账本在本事实表内）。
    pub const fn is_operator_command(self) -> bool {
        matches!(self, Self::OperatorAcknowledged)
    }
}

/// `const fn` 上下文中不能调用 `str::eq`，这里逐字节比较同一静态文本。
const fn is_same(value: &str, expected: &str) -> bool {
    let (left, right) = (value.as_bytes(), expected.as_bytes());
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}

/// 操作者来源闭集（决策 17）：Admin 共用身份、CLI 附带系统用户，Worker 与迁移如实标注。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeliveryDispositionActorSource {
    Admin,
    Cli,
    Worker,
    System,
}

impl DeliveryDispositionActorSource {
    /// 全部来源；持久化闭集必须逐项对应。
    pub const ALL: [Self; 4] = [Self::Admin, Self::Cli, Self::Worker, Self::System];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Cli => "cli",
            Self::Worker => "worker",
            Self::System => "system",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DeliveryDispositionError> {
        Self::ALL
            .into_iter()
            .find(|source| source.as_str() == value)
            .ok_or(DeliveryDispositionError::InvalidActor)
    }
}

/// 处置证据的唯一来源引用；每种处置恰好一个非空，预检退役没有外部引用。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeliveryDispositionSource {
    /// 人工 Sent 证明的命令 ID（`delivery_sent_confirmations.command_id`）。
    pub sent_confirmation_id: Option<Uuid>,
    /// Admin 对账判失败的账本命令 ID（`delivery_admin_commands.command_id`）。
    pub admin_command_id: Option<Uuid>,
    /// 装备损失命令 ID（`settlement_equipment_losses.command_id`）。
    pub equipment_loss_id: Option<Uuid>,
    /// 历史自寄修复命令 ID（`settlement_legacy_self_reconciliations.command_id`）。
    pub legacy_self_reconciliation_id: Option<Uuid>,
    /// 自动恢复的远端绑定 `(world_id, external_mail_id)`。
    pub sent_binding: Option<(String, String)>,
    /// COD 三次退回的恢复记录 ID（`settlement_recoveries.id`）。
    pub settlement_recovery_id: Option<Uuid>,
    /// 操作者确认写入的同事务审计 ID（`audit_records.id`）。
    pub audit_id: Option<Uuid>,
}

impl DeliveryDispositionSource {
    /// 已填写的引用个数；绑定按一个引用计。
    pub fn reference_count(&self) -> usize {
        usize::from(self.sent_confirmation_id.is_some())
            + usize::from(self.admin_command_id.is_some())
            + usize::from(self.equipment_loss_id.is_some())
            + usize::from(self.legacy_self_reconciliation_id.is_some())
            + usize::from(self.sent_binding.is_some())
            + usize::from(self.settlement_recovery_id.is_some())
            + usize::from(self.audit_id.is_some())
    }

    /// 校验引用与种类一一对应，且引用本身形状有效；不读取任何数据库事实。
    pub fn validate(&self, kind: DeliveryDispositionKind) -> Result<(), DeliveryDispositionError> {
        if let Some((world, external_mail_id)) = &self.sent_binding
            && (!matches!(world.as_str(), "main" | "isekai")
                || !valid_positive_decimal(external_mail_id, 64))
        {
            return Err(DeliveryDispositionError::InvalidSource);
        }
        let expected = usize::from(kind != DeliveryDispositionKind::PreflightRetired);
        if self.reference_count() != expected {
            return Err(DeliveryDispositionError::InvalidSource);
        }
        let matches_kind = match kind {
            DeliveryDispositionKind::AutoSentProof => self.sent_binding.is_some(),
            DeliveryDispositionKind::OperatorSentConfirmation => {
                self.sent_confirmation_id.is_some()
            }
            DeliveryDispositionKind::AdminFailed => self.admin_command_id.is_some(),
            DeliveryDispositionKind::EquipmentLoss => self.equipment_loss_id.is_some(),
            DeliveryDispositionKind::CodReturned => self.settlement_recovery_id.is_some(),
            DeliveryDispositionKind::LegacySelf => self.legacy_self_reconciliation_id.is_some(),
            DeliveryDispositionKind::OperatorAcknowledged => self.audit_id.is_some(),
            DeliveryDispositionKind::PreflightRetired => true,
        };
        if matches_kind { Ok(()) } else { Err(DeliveryDispositionError::InvalidSource) }
    }
}

/// 预检退役的失败码必须来自闭集；其余种类可带可选原因码。
pub fn validate_disposition_reason_code(
    kind: DeliveryDispositionKind,
    reason_code: Option<&str>,
) -> Result<(), DeliveryDispositionError> {
    match (kind, reason_code) {
        (DeliveryDispositionKind::PreflightRetired, None) => {
            Err(DeliveryDispositionError::InvalidReason)
        }
        (DeliveryDispositionKind::PreflightRetired, Some(code))
            if !PREFLIGHT_RETIREMENT_CODES.iter().any(|known| known == &code) =>
        {
            Err(DeliveryDispositionError::InvalidReason)
        }
        (_, Some(code)) if !valid_text(code, 64) => Err(DeliveryDispositionError::InvalidReason),
        _ => Ok(()),
    }
}

/// 操作者标识：非空、最多 128 字节、无首尾空白与控制字符。
pub fn validate_disposition_actor(actor_id: &str) -> Result<(), DeliveryDispositionError> {
    if valid_text(actor_id, 128) { Ok(()) } else { Err(DeliveryDispositionError::InvalidActor) }
}

/// 处置原因：必须填写时不得为空；已填写时必须规范且不超过 2048 字节。
pub fn validate_disposition_reason(
    kind: DeliveryDispositionKind,
    reason: Option<&str>,
) -> Result<(), DeliveryDispositionError> {
    match (kind.requires_operator_reason(), reason) {
        (true, None) => Err(DeliveryDispositionError::InvalidReason),
        (_, Some(reason)) if !valid_text(reason, 2048) => {
            Err(DeliveryDispositionError::InvalidReason)
        }
        _ => Ok(()),
    }
}

/// 确认“历史失败已处置”的准入事实；不含任何外部正文或凭据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliveryAcknowledgementFacts<'a> {
    /// 投递当前状态；只有 `failed` 可确认。
    pub status: &'a str,
    /// 投递来源种类；结算与恢复来源不允许。
    pub origin_kind: &'a str,
    /// 冻结载荷中的 Credits 附件金额；`None` 表示无法精确读取（保守拒绝）。
    pub credits: Option<i64>,
    /// 冻结载荷中的 COD 金额；`None` 表示无法精确读取（保守拒绝）。
    pub cod: Option<i64>,
    /// 冻结载荷中的附件装备数量。
    pub equipment_count: usize,
    /// 该投递是否仍绑定结算履约义务。
    pub has_fulfillment_obligation: bool,
}

/// 只允许“没有资金或资产影响”的历史失败被确认处置。
///
/// 该确认不改变投递状态、资金、履约或资产事实，只追加处置事实让读侧停止把它当作
/// 待处理失败；因此必须排除一切仍有资金或资产责任的条目。
pub fn validate_acknowledgement(
    facts: DeliveryAcknowledgementFacts<'_>,
) -> Result<(), DeliveryDispositionError> {
    if facts.status != "failed"
        || matches!(facts.origin_kind, "settlement" | "settlement_recovery_action")
        || facts.has_fulfillment_obligation
        || facts.equipment_count != 0
        || facts.credits != Some(0)
        || facts.cod != Some(0)
    {
        return Err(DeliveryDispositionError::NotAcknowledgeable);
    }
    Ok(())
}

/// 处置准入错误；只携带闭集分类，不包含业务正文。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DeliveryDispositionError {
    #[error("投递处置种类不在闭集内")]
    UnknownKind,
    #[error("投递处置来源引用必须恰好一个且与种类匹配")]
    InvalidSource,
    #[error("投递处置操作者来源或标识无效")]
    InvalidActor,
    #[error("投递处置原因或原因码无效")]
    InvalidReason,
    #[error("该投递没有确认已处置的资格")]
    NotAcknowledgeable,
}
