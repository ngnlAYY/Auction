// Admin 投递动作的准入规则：动作—状态迁移、发送目击、永久退役与冻结资金绑定。
//
// 存储层按既有锁序读取投递行、资金绑定、退役事实与 lot 发送资格，把结果投影成本结构；
// 本模块只做纯判定，不读数据库、不写日志，也不回写任何命令账本。阻断项是闭集：提交事务
// 取第一个阻断项映射回既有 `DeliveryAdminError`，因此判定顺序与旧内联分支保持一致。
//
// 受保护的不变量：
// - 已提交命令的重放/冲突先于业务判定，人工动作不得改写已绑定资金的出站责任；
// - Worker 已在完整 Sent 中目击同一义务时，重试或判失败都会重复付款或误判，必须失败关闭；
// - 已被装备损失或历史自寄核对永久退役的投递不接受任何新动作；
// - 每个动作只有一组允许的起止状态，账本回放复用同一映射。

use crate::ports::delivery::admin::{DeliveryAdminAction, DeliveryAdminError};

/// 单一动作唯一允许的起止状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliveryAdminTransition {
    pub expected_status: &'static str,
    pub next_status: &'static str,
}

impl DeliveryAdminAction {
    /// 返回动作唯一的起止状态；账本回放与提交事务必须共用该映射。
    pub const fn transition(self) -> DeliveryAdminTransition {
        let (expected_status, next_status) = match self {
            Self::Release => ("claimed", "pending"),
            Self::RetryFailed => ("failed", "pending"),
            Self::ReconcileRetry => ("manual_reconcile", "pending"),
            Self::ReconcileSucceeded => ("manual_reconcile", "succeeded"),
            Self::ReconcileFailed => ("manual_reconcile", "failed"),
        };
        DeliveryAdminTransition { expected_status, next_status }
    }
}

/// Admin 动作判定所需的已锁定事实；全部字段由提交事务在同一锁定范围内读取。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryAdminGuardSnapshot {
    pub action: DeliveryAdminAction,
    pub expected_fencing_version: i64,
    pub fencing_version: i64,
    pub status: String,
    /// 冻结载荷的发送原因；`mail_disposition_conflict` 表示发送前已被隔离。
    pub reason_code: Option<String>,
    /// `settlement` 等来源种类；只有结算来源才可能有 Sent 目击。
    pub origin_kind: String,
    /// 冻结载荷的 `operation`；非 `send` 载荷不得借人工对账伪装外部成功。
    pub payload_operation: Option<String>,
    /// Worker 已在完整 Sent 中目击同一结算义务的邮件。
    pub sent_reference_sighted: bool,
    /// 该投递已绑定资金 allocation；释放或重试会改写资金责任。
    pub funded_delivery: bool,
    /// 已被装备损失或历史自寄核对永久退役，任何新动作都不能重新激活。
    pub retired: bool,
    /// 只有 `Release`/`RetryFailed`/`ReconcileRetry` 读取 lot 发送资格；其余动作为 `None`。
    pub lot_eligible: Option<bool>,
    /// `ReconcileSucceeded` 是否提供了外部引用（`Some`，空串与旧版一致仍交给后续校验）。
    pub has_reconcile_reference: bool,
}

/// 阻断项闭集；顺序即判定优先级，调用方取第一项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryAdminGuardBlocker {
    /// 已绑定资金责任，禁止人工释放/重试/对账改写。
    FundedDelivery,
    /// 已被永久退役事实收口。
    RetiredDelivery,
    /// 预期 fencing version 已过期。
    StaleFencingVersion,
    /// Sent 已目击同一义务，须走人工 Sent 证明。
    SentSighted,
    /// `send` 载荷不允许被 Admin 直接对账为成功。
    ReconcileSucceededForSend,
    /// lot 已不再满足发送资格（提醒/成交被取消或重命名）。
    DeliveryNotEligible,
    /// 当前状态不属于该动作的允许起始状态。
    StatusMismatch,
    /// 发送前已隔离的邮件只允许终结意图，不允许重试或伪装成功。
    QuarantinedMailActionNotAllowed,
    /// 对账成功缺少外部引用，不能形成可核验的人工证明。
    MissingReconcileReference,
}

impl DeliveryAdminGuardBlocker {
    /// 稳定标识；只用于内部审计与测试断言，不进入玩家可见契约。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FundedDelivery => "funded_delivery",
            Self::RetiredDelivery => "retired_delivery",
            Self::StaleFencingVersion => "stale_fencing_version",
            Self::SentSighted => "sent_sighted",
            Self::ReconcileSucceededForSend => "reconcile_succeeded_for_send",
            Self::DeliveryNotEligible => "delivery_not_eligible",
            Self::StatusMismatch => "status_mismatch",
            Self::QuarantinedMailActionNotAllowed => "quarantined_mail_action_not_allowed",
            Self::MissingReconcileReference => "missing_reconcile_reference",
        }
    }

    /// 映射回既有 `DeliveryAdminError`；不引入新的外部错误类别。
    pub const fn error(self) -> DeliveryAdminError {
        match self {
            Self::FundedDelivery
            | Self::RetiredDelivery
            | Self::ReconcileSucceededForSend
            | Self::QuarantinedMailActionNotAllowed
            | Self::MissingReconcileReference => DeliveryAdminError::Invalid,
            Self::SentSighted => DeliveryAdminError::SentSighted,
            Self::StaleFencingVersion | Self::DeliveryNotEligible | Self::StatusMismatch => {
                DeliveryAdminError::Stale
            }
        }
    }
}

/// 判定结果；`result_metadata` 只在无阻断项时由提交事务写入投递行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryAdminGuardDecision {
    pub transition: DeliveryAdminTransition,
    pub blockers: Vec<DeliveryAdminGuardBlocker>,
    pub result_metadata: serde_json::Value,
}

impl DeliveryAdminGuardDecision {
    pub fn eligible(&self) -> bool {
        self.blockers.is_empty()
    }
}

/// 是否需要对 lot 读取发送资格的动作。
const fn checks_lot_eligibility(action: DeliveryAdminAction) -> bool {
    matches!(
        action,
        DeliveryAdminAction::Release
            | DeliveryAdminAction::RetryFailed
            | DeliveryAdminAction::ReconcileRetry
    )
}

/// 是否需要在 Sent 目击时失败关闭的动作。
const fn checks_sent_sighting(action: DeliveryAdminAction) -> bool {
    matches!(action, DeliveryAdminAction::ReconcileRetry | DeliveryAdminAction::ReconcileFailed)
}

impl DeliveryAdminGuardSnapshot {
    /// 计算唯一判定；阻断项按旧内联分支顺序排列，调用方取第一项即恢复原错误优先级。
    pub fn evaluate(&self) -> DeliveryAdminGuardDecision {
        let transition = self.action.transition();
        let mut blockers = Vec::new();
        if self.funded_delivery {
            blockers.push(DeliveryAdminGuardBlocker::FundedDelivery);
        }
        if self.retired {
            blockers.push(DeliveryAdminGuardBlocker::RetiredDelivery);
        }
        if self.fencing_version != self.expected_fencing_version {
            blockers.push(DeliveryAdminGuardBlocker::StaleFencingVersion);
        }
        if checks_sent_sighting(self.action)
            && self.origin_kind == "settlement"
            && self.payload_operation.as_deref() == Some("send")
            && self.sent_reference_sighted
        {
            blockers.push(DeliveryAdminGuardBlocker::SentSighted);
        }
        if self.action == DeliveryAdminAction::ReconcileSucceeded
            && self.payload_operation.as_deref() == Some("send")
        {
            // 裸 external_reference 不是 Sent 内容证明；人工对账不能绕过主动寄送的成功门。
            blockers.push(DeliveryAdminGuardBlocker::ReconcileSucceededForSend);
        }
        if checks_lot_eligibility(self.action) && self.lot_eligible == Some(false) {
            blockers.push(DeliveryAdminGuardBlocker::DeliveryNotEligible);
        }
        if self.status != transition.expected_status {
            blockers.push(DeliveryAdminGuardBlocker::StatusMismatch);
        }
        let quarantined_mail = self.reason_code.as_deref() == Some("mail_disposition_conflict")
            && self.action != DeliveryAdminAction::ReconcileFailed;
        if quarantined_mail {
            // 隔离发生在发送前；旧 payload 没有重新发送或伪装外部成功的授权。
            blockers.push(DeliveryAdminGuardBlocker::QuarantinedMailActionNotAllowed);
        }
        if self.action == DeliveryAdminAction::ReconcileSucceeded && !self.has_reconcile_reference {
            blockers.push(DeliveryAdminGuardBlocker::MissingReconcileReference);
        }
        let result_metadata = if blockers.is_empty() {
            match self.action {
                DeliveryAdminAction::ReconcileSucceeded => {
                    serde_json::json!({"resolution":"admin_reconciled_succeeded"})
                }
                DeliveryAdminAction::ReconcileFailed => {
                    if self.reason_code.as_deref() == Some("mail_disposition_conflict") {
                        serde_json::json!({
                            "resolution": "admin_reconciled_failed",
                            "reason_code": "mail_disposition_conflict",
                            "preflight_blocked": true,
                            "outcome_unknown": false,
                        })
                    } else {
                        serde_json::json!({"resolution":"admin_reconciled_failed"})
                    }
                }
                _ => serde_json::json!({}),
            }
        } else {
            serde_json::json!({})
        };
        DeliveryAdminGuardDecision { transition, blockers, result_metadata }
    }
}
