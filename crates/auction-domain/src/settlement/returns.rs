//! 结算退件处理的纯规则：案件状态与 Admin 动作闭集、每案冻结的补寄上限、动作准入判定，以及
//! 补寄派发的稳定身份与正文派发引用格式。
//!
//! 案件与经济义务分开：案件终结只表示这一封退件的处理已交接或完成；原义务是否满足、资产归谁、
//! 是否仍欠款分别判断。代管结案不消灭原权利人的应收，也不把资产记为 Bot 自有。

use uuid::Uuid;

use super::SettlementObligationKind;
use crate::identity::legacy_natural_key_uuid;

/// 每个退件案件冻结的补寄上限（不含首寄）。案件建立时写入案件行；以后调整上限只影响新建
/// 案件，已冻结的案件不改写（不变量 10）。
pub const SETTLEMENT_RETURN_RESEND_LIMIT: u8 = 3;

/// 首寄的派发编号；补寄从 2 起逐次递增，`(义务, 派发编号)` 全局唯一。
pub const FIRST_DISPATCH_NO: u32 = 1;

/// 补寄正文末行派发引用的固定前缀；首寄正文没有这一行。
pub const DISPATCH_REFERENCE_LINE_PREFIX: &str = "Dispatch reference: ";

/// 派发引用格式版本；改变编码必须换版本，旧版本仍按原规则解析。
const DISPATCH_REFERENCE_VERSION: &str = "v1";

const RESEND_DELIVERY_NAMESPACE: &str = "auction.fulfillment.resend.delivery.v1";
const RESEND_PREPARE_NAMESPACE: &str = "auction.fulfillment.resend.prepare.v1";
const RESEND_ACTIVATE_NAMESPACE: &str = "auction.fulfillment.resend.activate.v1";

/// 补寄投递的稳定 ID：首寄沿用旧单例算法（不在此派生），补寄按义务与派发编号进入独立命名空间，
/// 同一派发重放得到同一身份。派发编号小于 2 时返回 `None`。
pub fn resend_delivery_id(obligation_id: Uuid, dispatch_no: u32) -> Option<Uuid> {
    resend_identity(RESEND_DELIVERY_NAMESPACE, obligation_id, dispatch_no)
}

/// 补寄 prepare 命令的稳定 ID；规则同 [`resend_delivery_id`]。
pub fn resend_prepare_command_id(obligation_id: Uuid, dispatch_no: u32) -> Option<Uuid> {
    resend_identity(RESEND_PREPARE_NAMESPACE, obligation_id, dispatch_no)
}

/// 补寄 activate 命令的稳定 ID；规则同 [`resend_delivery_id`]。
pub fn resend_activate_command_id(obligation_id: Uuid, dispatch_no: u32) -> Option<Uuid> {
    resend_identity(RESEND_ACTIVATE_NAMESPACE, obligation_id, dispatch_no)
}

fn resend_identity(namespace: &str, obligation_id: Uuid, dispatch_no: u32) -> Option<Uuid> {
    (dispatch_no > FIRST_DISPATCH_NO && !obligation_id.is_nil()).then(|| {
        legacy_natural_key_uuid(namespace, &[obligation_id.as_bytes(), &dispatch_no.to_be_bytes()])
    })
}

/// 补寄冻结正文里的版本化派发引用 `v1/<派发编号>/<补寄投递 ID>`。系统退回原样带回正文，
/// 读取退件时据此把退件绑定到具体派发；投递 ID 由义务与编号确定，换义务不能伪造同一引用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettlementDispatchReference {
    pub dispatch_no: u32,
    pub delivery_id: Uuid,
}

impl SettlementDispatchReference {
    /// 为补寄派发构造引用；首寄没有引用。
    pub fn for_resend(obligation_id: Uuid, dispatch_no: u32) -> Option<Self> {
        resend_delivery_id(obligation_id, dispatch_no)
            .map(|delivery_id| Self { dispatch_no, delivery_id })
    }

    /// 写入载荷变量与正文末行的规范文本。
    pub fn encode(&self) -> String {
        format!("{DISPATCH_REFERENCE_VERSION}/{}/{}", self.dispatch_no, self.delivery_id)
    }

    /// 只接受 [`Self::encode`] 的规范输出：版本固定、编号为不带前导零的 2 以上十进制、投递 ID
    /// 为规范小写 UUID；其余一律返回 `None`。
    pub fn parse(value: &str) -> Option<Self> {
        let mut parts = value.split('/');
        let (Some(version), Some(number), Some(delivery), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return None;
        };
        if version != DISPATCH_REFERENCE_VERSION
            || number.is_empty()
            || number.starts_with('0')
            || !number.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        let dispatch_no = number.parse::<u32>().ok().filter(|value| *value > FIRST_DISPATCH_NO)?;
        let delivery_id = Uuid::parse_str(delivery).ok()?;
        (!delivery_id.is_nil() && delivery_id.to_string() == delivery)
            .then_some(Self { dispatch_no, delivery_id })
    }

    /// 引用是否正是该义务这一派发编号的补寄身份。
    pub fn belongs_to(&self, obligation_id: Uuid) -> bool {
        resend_delivery_id(obligation_id, self.dispatch_no) == Some(self.delivery_id)
    }
}

/// 义务种类是否支持补寄：卖方货款、买方退款、流拍与撤拍寄回。付款 COD 与异世界交付归恢复
/// 流程处理，普通退件不得为它们重寄、取消成交或拉黑。
pub const fn resend_supported(kind: SettlementObligationKind) -> bool {
    matches!(
        kind,
        SettlementObligationKind::SellerPayout
            | SettlementObligationKind::BuyerRefund
            | SettlementObligationKind::UnsoldReturn
            | SettlementObligationKind::WithdrawnReturn
    )
}

/// 退回资产是否为现金（领回后经资金返还恢复原用途）。
pub const fn cash_return(kind: SettlementObligationKind) -> bool {
    matches!(kind, SettlementObligationKind::SellerPayout | SettlementObligationKind::BuyerRefund)
}

macro_rules! closed_names {
    (
        $(#[$meta:meta])*
        pub enum $name:ident { $($(#[$variant_meta:meta])* $variant:ident => $value:literal,)+ }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name { $($(#[$variant_meta])* $variant,)+ }

        impl $name {
            /// 全部取值，按声明顺序。
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];

            /// 持久化与契约使用的稳定名称。
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $value,)+ }
            }

            /// 只接受闭集名称；未知值返回 `None`，由调用方失败关闭。
            pub fn from_name(value: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|item| item.as_str() == value)
            }
        }
    };
}

closed_names! {
    /// 退件案件状态闭集，即 `settlement_return_cases.status`。
    pub enum SettlementReturnCaseStatus {
        /// 来源未证明（`untrusted`）或未唯一绑定原派发（`unbound`）：隔离待核对。
        Quarantined => "quarantined",
        /// 未绑定案件已提交只读重新核验，等待 Worker 产生核验事实。
        RecheckPending => "recheck_pending",
        /// 已绑定原派发，资产仍在 Bot 收件箱（退回待领取），等待 Admin 决定。
        AwaitingDecision => "awaiting_decision",
        /// 持久 Take 已提交，等待外部执行与领取证明。
        TakePending => "take_pending",
        /// Take 结果未知或失败：停在人工核对，只认系统可验证证明，不自动重试。
        TakeUnknown => "take_unknown",
        /// 已领回，原义务的新派发正在准备或寄送。
        ResendPending => "resend_pending",
        /// 已领回，替原权利人代管（权益保留，可再寄）。
        Held => "held",
        /// 新派发的寄出证明成立，本案结束；原义务仍等待收件人领取。
        Resent => "resent",
        /// 明确例外：不领取，资产留在收件箱记为退回待领取，权益保留。
        InboxClosed => "inbox_closed",
        /// 隔离记录经核对结案；不授予任何资产或资金动作。
        Reviewed => "reviewed",
    }
}

impl SettlementReturnCaseStatus {
    /// 案件是否仍需要处理（计入待办与告警）。代管视为已结案：权益与资产位置由对账单列。
    pub const fn open(self) -> bool {
        !matches!(self, Self::Held | Self::Resent | Self::InboxClosed | Self::Reviewed)
    }
}

closed_names! {
    /// Admin 处理动作闭集；资产、金额与收件人都由服务端冻结事实决定，不接收任意载荷。
    pub enum SettlementReturnAction {
        /// 领回后在原义务上补寄给原冻结收件人。
        ReclaimAndResend => "reclaim_and_resend",
        /// 领回后替原权利人代管结案。
        ReclaimAndHold => "reclaim_and_hold",
        /// 从代管状态补寄（领回证明已持久化）。
        ResendFromCustody => "resend_from_custody",
        /// 明确例外：不领取，留箱结案。
        CloseInbox => "close_inbox",
        /// 隔离记录标记已核对。
        Acknowledge => "acknowledge",
        /// 未绑定记录提交 Worker 只读重新核验。
        Recheck => "recheck",
    }
}

closed_names! {
    /// 领回后的处理计划，即 `settlement_return_cases.plan`。
    pub enum SettlementReturnPlan {
        Resend => "resend",
        Hold => "hold",
    }
}

closed_names! {
    /// 动作被拒绝的类型化原因；HTTP 映射为各自的 409 错误码。
    pub enum SettlementReturnBlocker {
        /// 来源未证明：只能标记已核对。
        Untrusted => "untrusted",
        /// 未唯一绑定原派发：只能标记已核对或重新核验。
        Unbound => "unbound",
        /// 退回资产与原派发冻结资产不符，不能领取或补寄。
        AssetsMismatch => "assets_mismatch",
        /// 案件当前状态不允许该动作（处理中、已结案或动作不适用）。
        StateConflict => "state_conflict",
        /// 补寄次数已达案件冻结上限，只能代管结案。
        ResendLimit => "resend_limit",
        /// 原义务已不可补寄：成交撤销、已满足或终结、非当前派发、种类不支持或历史现金无资金分配。
        ObligationIneligible => "obligation_ineligible",
        /// 需要已持久化的领回证明。
        ProofRequired => "proof_required",
        /// 原派发已占用资金分配，但发送结果仍未核实（分配不是 spent）：须先经 Sent 恢复或人工
        /// Sent 证明收口原发送，才能领回、留箱或补寄；否则结案会关闭原发送的 Sent 恢复入口，
        /// 留下永远无法收口的未知占用。
        SendUnresolved => "send_unresolved",
    }
}

/// 证据绑定强度，由 `settlement_return_evidence.match_state` 投影。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementReturnBinding {
    Untrusted,
    Unbound,
    Bound,
}

/// 原义务与退件派发的关系，由存储在锁内复核得出。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementReturnObligationPosition {
    /// 义务仍可执行，退件派发正是履约当前派发，履约已转人工对账（或代管时为确定终结）。
    Current,
    /// 义务已满足（收件人领取过某次派发）。
    Satisfied,
    /// 成交撤销或履约已确定失败等业务终结。
    Ended,
    /// 退件派发已不是当前派发，或履约事实缺失。
    Superseded,
}

/// 现金退件的资金关联。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementReturnFunding {
    /// 装备类退件，不涉及现金用途。
    NotCash,
    /// 原派发绑定了资金分配：领回证明成立后经返还恢复原用途。
    Funded,
    /// 历史现金退件没有原资金分配：只能代管结案，不支持补寄。
    Unfunded,
    /// 原派发占用过资金分配（当前指针或只追加的历史绑定），但既没有 spent 支出也没有本案返还：
    /// 原发送结果未知。与“历史没有分配”不同，必须先由 Sent 证明收口原发送。
    Unresolved,
}

/// 动作准入所需的已锁定事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettlementReturnAdmission {
    pub status: SettlementReturnCaseStatus,
    pub binding: SettlementReturnBinding,
    /// 退回资产与原派发冻结资产逐项一致。
    pub assets_match: bool,
    pub obligation_kind: Option<SettlementObligationKind>,
    pub position: SettlementReturnObligationPosition,
    pub funding: SettlementReturnFunding,
    /// 该义务已经发生的补寄次数（当前派发编号减一）。
    pub resends_used: u32,
    /// 案件冻结的补寄上限。
    pub resend_limit: u8,
    /// 本案领回证明已持久化。
    pub take_proven: bool,
}

impl SettlementReturnAdmission {
    /// 判定单个动作；拒绝时返回唯一的类型化原因。顺序固定：先来源与绑定，再状态，再原发送的资金
    /// 是否已核实，再资产，最后义务资格与次数，使同一事实总得到同一错误码。
    pub fn check(&self, action: SettlementReturnAction) -> Result<(), SettlementReturnBlocker> {
        use SettlementReturnAction as A;
        use SettlementReturnBlocker as B;
        use SettlementReturnCaseStatus as S;

        match action {
            A::Acknowledge => {
                if self.binding == SettlementReturnBinding::Bound {
                    // 已绑定退件的资产仍在收件箱；不处置就结案只能走留箱例外。
                    return Err(B::StateConflict);
                }
                if self.status != S::Quarantined {
                    return Err(B::StateConflict);
                }
                Ok(())
            }
            A::Recheck => match self.binding {
                SettlementReturnBinding::Untrusted => Err(B::Untrusted),
                SettlementReturnBinding::Bound => Err(B::StateConflict),
                SettlementReturnBinding::Unbound if self.status == S::Quarantined => Ok(()),
                SettlementReturnBinding::Unbound => Err(B::StateConflict),
            },
            A::CloseInbox | A::ReclaimAndHold | A::ReclaimAndResend => {
                self.require_bound()?;
                if self.status != S::AwaitingDecision {
                    return Err(B::StateConflict);
                }
                // 原发送未知时任何处置都会终结履约、关闭原发送的 Sent 恢复入口；留箱同样拒绝。
                if self.funding == SettlementReturnFunding::Unresolved {
                    return Err(B::SendUnresolved);
                }
                if action == A::CloseInbox {
                    return Ok(());
                }
                if !self.assets_match {
                    return Err(B::AssetsMismatch);
                }
                if action == A::ReclaimAndResend {
                    self.resend_allowed()?;
                }
                Ok(())
            }
            A::ResendFromCustody => {
                self.require_bound()?;
                if self.status != S::Held {
                    return Err(B::StateConflict);
                }
                if !self.take_proven {
                    return Err(B::ProofRequired);
                }
                self.resend_allowed()
            }
        }
    }

    /// 当前允许的动作集合，按闭集声明顺序。
    pub fn allowed_actions(&self) -> Vec<SettlementReturnAction> {
        SettlementReturnAction::ALL
            .iter()
            .copied()
            .filter(|action| self.check(*action).is_ok())
            .collect()
    }

    /// 补寄资格：原发送资金已核实、义务种类支持、仍是当前派发且可执行、现金有原资金分配、次数
    /// 未达冻结上限。
    pub fn resend_allowed(&self) -> Result<(), SettlementReturnBlocker> {
        if self.funding == SettlementReturnFunding::Unresolved {
            return Err(SettlementReturnBlocker::SendUnresolved);
        }
        let supported = self.obligation_kind.is_some_and(resend_supported);
        if !supported
            || self.position != SettlementReturnObligationPosition::Current
            || self.funding == SettlementReturnFunding::Unfunded
            || (self.obligation_kind.is_some_and(cash_return)
                && self.funding != SettlementReturnFunding::Funded)
        {
            return Err(SettlementReturnBlocker::ObligationIneligible);
        }
        if self.resends_used >= u32::from(self.resend_limit) {
            return Err(SettlementReturnBlocker::ResendLimit);
        }
        Ok(())
    }

    fn require_bound(&self) -> Result<(), SettlementReturnBlocker> {
        match self.binding {
            SettlementReturnBinding::Untrusted => Err(SettlementReturnBlocker::Untrusted),
            SettlementReturnBinding::Unbound => Err(SettlementReturnBlocker::Unbound),
            SettlementReturnBinding::Bound => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{
        FIRST_DISPATCH_NO, SETTLEMENT_RETURN_RESEND_LIMIT, SettlementDispatchReference,
        SettlementReturnAction, SettlementReturnAdmission, SettlementReturnBinding,
        SettlementReturnBlocker, SettlementReturnCaseStatus, SettlementReturnFunding,
        SettlementReturnObligationPosition, resend_activate_command_id, resend_delivery_id,
        resend_prepare_command_id,
    };
    use crate::SettlementObligationKind;

    const OBLIGATION: Uuid = Uuid::from_u128(0x5f6e_1c2d_3b4a_4c5d_8e7f_6a5b_4c3d_2e1f);

    fn bound(kind: SettlementObligationKind) -> SettlementReturnAdmission {
        SettlementReturnAdmission {
            status: SettlementReturnCaseStatus::AwaitingDecision,
            binding: SettlementReturnBinding::Bound,
            assets_match: true,
            obligation_kind: Some(kind),
            position: SettlementReturnObligationPosition::Current,
            funding: if super::cash_return(kind) {
                SettlementReturnFunding::Funded
            } else {
                SettlementReturnFunding::NotCash
            },
            resends_used: 0,
            resend_limit: SETTLEMENT_RETURN_RESEND_LIMIT,
            take_proven: false,
        }
    }

    /// 首寄不派生补寄身份；补寄身份按编号区分、三种身份互不相同，且固定为冻结向量。
    #[test]
    fn resend_identities_are_stable_per_dispatch() {
        assert_eq!(resend_delivery_id(OBLIGATION, FIRST_DISPATCH_NO), None);
        assert_eq!(resend_delivery_id(Uuid::nil(), 2), None);
        let second = resend_delivery_id(OBLIGATION, 2);
        let third = resend_delivery_id(OBLIGATION, 3);
        assert!(second.is_some() && third.is_some() && second != third);
        assert_ne!(resend_prepare_command_id(OBLIGATION, 2), second);
        assert_ne!(resend_activate_command_id(OBLIGATION, 2), second);
        assert_ne!(
            resend_prepare_command_id(OBLIGATION, 2),
            resend_activate_command_id(OBLIGATION, 2)
        );
        assert_eq!(resend_delivery_id(OBLIGATION, 2), second, "重放得到同一身份");
        assert_eq!(
            second.map(|value| value.to_string()).as_deref(),
            Some("dcd5da17-137d-5477-8aea-8a2baf43713f"),
            "补寄投递身份字节兼容，不可修改"
        );
    }

    /// 派发引用只接受规范编码，并且只属于派生它的义务。
    #[test]
    fn dispatch_reference_round_trips_and_rejects_non_canonical_text() {
        let reference = SettlementDispatchReference::for_resend(OBLIGATION, 2).expect("补寄引用");
        let encoded = reference.encode();
        assert_eq!(SettlementDispatchReference::parse(&encoded), Some(reference));
        assert!(reference.belongs_to(OBLIGATION));
        assert!(!reference.belongs_to(Uuid::from_u128(7)));
        assert_eq!(SettlementDispatchReference::for_resend(OBLIGATION, FIRST_DISPATCH_NO), None);
        let delivery = reference.delivery_id;
        for invalid in [
            format!("v2/2/{delivery}"),
            format!("v1/02/{delivery}"),
            format!("v1/1/{delivery}"),
            format!("v1/+2/{delivery}"),
            format!("v1/2/{}", delivery.to_string().to_uppercase()),
            format!("v1/2/{delivery}/x"),
            format!("v1/2/{}", Uuid::nil()),
            "v1/2".to_owned(),
            String::new(),
        ] {
            assert_eq!(SettlementDispatchReference::parse(&invalid), None, "{invalid}");
        }
    }

    /// 隔离记录：untrusted 只能核对；unbound 可核对或重新核验；都不能领取或补寄。
    #[test]
    fn quarantined_cases_only_allow_review_actions() {
        let mut untrusted = bound(SettlementObligationKind::SellerPayout);
        untrusted.status = SettlementReturnCaseStatus::Quarantined;
        untrusted.binding = SettlementReturnBinding::Untrusted;
        assert_eq!(untrusted.allowed_actions(), [SettlementReturnAction::Acknowledge]);
        assert_eq!(
            untrusted.check(SettlementReturnAction::ReclaimAndHold),
            Err(SettlementReturnBlocker::Untrusted)
        );
        assert_eq!(
            untrusted.check(SettlementReturnAction::Recheck),
            Err(SettlementReturnBlocker::Untrusted)
        );
        let mut unbound = untrusted;
        unbound.binding = SettlementReturnBinding::Unbound;
        assert_eq!(
            unbound.allowed_actions(),
            [SettlementReturnAction::Acknowledge, SettlementReturnAction::Recheck]
        );
        assert_eq!(
            unbound.check(SettlementReturnAction::ReclaimAndResend),
            Err(SettlementReturnBlocker::Unbound)
        );
    }

    /// 已绑定待决定：领回补寄、领回代管、留箱结案三选一；已绑定不能直接“核对”结案。
    #[test]
    fn bound_cases_choose_between_resend_hold_and_inbox() {
        let facts = bound(SettlementObligationKind::BuyerRefund);
        assert_eq!(
            facts.allowed_actions(),
            [
                SettlementReturnAction::ReclaimAndResend,
                SettlementReturnAction::ReclaimAndHold,
                SettlementReturnAction::CloseInbox
            ]
        );
        let mut mismatch = facts;
        mismatch.assets_match = false;
        assert_eq!(
            mismatch.check(SettlementReturnAction::ReclaimAndHold),
            Err(SettlementReturnBlocker::AssetsMismatch)
        );
        assert_eq!(mismatch.check(SettlementReturnAction::CloseInbox), Ok(()));
        let mut taking = facts;
        taking.status = SettlementReturnCaseStatus::TakePending;
        assert!(taking.allowed_actions().is_empty(), "处理中不接受新动作");
    }

    /// 补寄资格：种类、当前派发、现金资金分配与冻结上限分别给出确定原因。
    #[test]
    fn resend_requires_supported_current_funded_obligation_within_limit() {
        let mut cod = bound(SettlementObligationKind::WinnerEquipmentCod);
        cod.funding = SettlementReturnFunding::NotCash;
        assert_eq!(
            cod.check(SettlementReturnAction::ReclaimAndResend),
            Err(SettlementReturnBlocker::ObligationIneligible)
        );
        assert_eq!(cod.check(SettlementReturnAction::ReclaimAndHold), Ok(()));

        let mut unfunded = bound(SettlementObligationKind::SellerPayout);
        unfunded.funding = SettlementReturnFunding::Unfunded;
        assert_eq!(
            unfunded.check(SettlementReturnAction::ReclaimAndResend),
            Err(SettlementReturnBlocker::ObligationIneligible),
            "历史现金退件没有资金分配时只能代管"
        );
        assert_eq!(unfunded.check(SettlementReturnAction::ReclaimAndHold), Ok(()));

        for position in [
            SettlementReturnObligationPosition::Satisfied,
            SettlementReturnObligationPosition::Ended,
            SettlementReturnObligationPosition::Superseded,
        ] {
            let mut ended = bound(SettlementObligationKind::UnsoldReturn);
            ended.position = position;
            assert_eq!(
                ended.check(SettlementReturnAction::ReclaimAndResend),
                Err(SettlementReturnBlocker::ObligationIneligible),
                "{position:?}"
            );
        }

        let mut exhausted = bound(SettlementObligationKind::WithdrawnReturn);
        exhausted.resends_used = u32::from(SETTLEMENT_RETURN_RESEND_LIMIT);
        assert_eq!(
            exhausted.check(SettlementReturnAction::ReclaimAndResend),
            Err(SettlementReturnBlocker::ResendLimit)
        );
        exhausted.resends_used -= 1;
        assert_eq!(exhausted.check(SettlementReturnAction::ReclaimAndResend), Ok(()));
    }

    /// 原派发占用过资金但发送结果未知：领回补寄、领回代管与留箱都拒绝，原因固定为
    /// `send_unresolved`，与“历史没有分配”（只允许代管）区分；状态冲突仍优先。
    #[test]
    fn unresolved_original_send_blocks_every_disposal() {
        for kind in [SettlementObligationKind::SellerPayout, SettlementObligationKind::BuyerRefund]
        {
            let mut unresolved = bound(kind);
            unresolved.funding = SettlementReturnFunding::Unresolved;
            for action in [
                SettlementReturnAction::ReclaimAndResend,
                SettlementReturnAction::ReclaimAndHold,
                SettlementReturnAction::CloseInbox,
            ] {
                assert_eq!(
                    unresolved.check(action),
                    Err(SettlementReturnBlocker::SendUnresolved),
                    "{kind:?} {action:?}"
                );
            }
            assert!(unresolved.allowed_actions().is_empty(), "{kind:?}");
            let mut taking = unresolved;
            taking.status = SettlementReturnCaseStatus::TakePending;
            assert_eq!(
                taking.check(SettlementReturnAction::ReclaimAndHold),
                Err(SettlementReturnBlocker::StateConflict)
            );
            let mut held = unresolved;
            held.status = SettlementReturnCaseStatus::Held;
            held.take_proven = true;
            assert_eq!(
                held.check(SettlementReturnAction::ResendFromCustody),
                Err(SettlementReturnBlocker::SendUnresolved)
            );
            assert_eq!(unresolved.resend_allowed(), Err(SettlementReturnBlocker::SendUnresolved));
        }
        let mut unfunded = bound(SettlementObligationKind::SellerPayout);
        unfunded.funding = SettlementReturnFunding::Unfunded;
        assert_eq!(
            unfunded.allowed_actions(),
            [SettlementReturnAction::ReclaimAndHold, SettlementReturnAction::CloseInbox],
            "历史没有分配仍可代管或留箱"
        );
    }

    /// 代管后可再寄，但必须已有领回证明，且仍受资格与上限约束。
    #[test]
    fn custody_resend_requires_persisted_take_proof() {
        let mut held = bound(SettlementObligationKind::SellerPayout);
        held.status = SettlementReturnCaseStatus::Held;
        assert_eq!(
            held.check(SettlementReturnAction::ResendFromCustody),
            Err(SettlementReturnBlocker::ProofRequired)
        );
        held.take_proven = true;
        assert_eq!(held.allowed_actions(), [SettlementReturnAction::ResendFromCustody]);
        held.resends_used = 3;
        assert_eq!(
            held.check(SettlementReturnAction::ResendFromCustody),
            Err(SettlementReturnBlocker::ResendLimit)
        );
    }

    /// 闭集名称往返；未知名称失败关闭。
    #[test]
    fn closed_names_round_trip() {
        for status in SettlementReturnCaseStatus::ALL {
            assert_eq!(SettlementReturnCaseStatus::from_name(status.as_str()), Some(*status));
        }
        for action in SettlementReturnAction::ALL {
            assert_eq!(SettlementReturnAction::from_name(action.as_str()), Some(*action));
        }
        assert_eq!(SettlementReturnAction::from_name("force_bind"), None);
        assert!(SettlementReturnCaseStatus::TakeUnknown.open());
        assert!(!SettlementReturnCaseStatus::Held.open());
    }
}
