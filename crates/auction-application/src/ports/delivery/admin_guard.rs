//! Admin 投递动作的准入规则：动作—状态迁移、发送目击、永久退役、冻结资金绑定与结算义务资格。
//!
//! 存储层按既有锁序读取投递行、资金绑定、退役事实、lot 发送资格与结算义务资格，把结果投影成
//! 本结构；
//! 本模块只做纯判定，不读数据库、不写日志，也不回写任何命令账本。阻断项是闭集：提交事务
//! 取第一个阻断项映射回既有 `DeliveryAdminError`，因此判定顺序与旧内联分支保持一致。
//!
//! 受保护的不变量：
//! - 已提交命令的重放/冲突先于业务判定，人工动作不得改写已绑定资金的出站责任；
//! - 成交已撤销（或投递已不是义务当前派发）的结算投递不能被释放或重试重新激活，该阻断优先于
//!   其余阻断，给出明确业务错误；
//! - Worker 已在完整 Sent 中目击同一义务时，重试或判失败都会重复付款或误判，必须失败关闭；
//! - 已被装备损失或历史自寄核对永久退役的投递不接受任何新动作；
//! - 同一邮件已由结算退件 Take 接手（另一 owner 的意图被确定为从未执行后交接）时，原领取或退回
//!   意图不能被普通重试或释放复活，否则两个 owner 会对同一封邮件各执行一次；
//! - 每个动作只有一组允许的起止状态，账本回放复用同一映射。

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
    /// 当前 fence 已进入 sending；普通对账终态需要真实 sending 时间。
    pub has_sending_fence: bool,
    /// `settlement` 等来源种类；只有结算来源才可能有 Sent 目击。
    pub origin_kind: String,
    /// 冻结载荷的 `operation`；非 `send` 载荷不得借人工对账伪装外部成功。
    pub payload_operation: Option<String>,
    /// Worker 已在完整 Sent 中目击同一结算义务的邮件。
    pub sent_reference_sighted: bool,
    /// 该投递已绑定资金 allocation；释放或重试会改写资金责任。
    pub funded_delivery: bool,
    /// 释放、重排和对账使用的完整命令快照已通过校验。
    pub command_snapshot_readable: bool,
    /// 已由装备损失、历史自寄核对或预检退役收口，任何新动作都不能重新激活。
    pub retired: bool,
    /// 只有 `Release`/`RetryFailed`/`ReconcileRetry` 读取 lot 发送资格；其余动作为 `None`。
    pub lot_eligible: Option<bool>,
    /// 同样只有这三个动作读取：结算来源投递的义务是否仍可执行且本投递是其当前派发；非结算来源
    /// 或其余动作为 `None`。
    pub settlement_eligible: Option<bool>,
    /// 该投递占用的履约行状态；`None` 表示没有绑定的履约行。
    pub settlement_fulfillment_state: Option<String>,
    /// `ReconcileSucceeded` 是否提供了外部引用（`Some`，空串与旧版一致仍交给后续校验）。
    pub has_reconcile_reference: bool,
    /// 本投递是入站或恢复对某封邮件的领取/退回意图，而同一邮件已有结算退件 Take（交接之后）。
    pub mail_handed_over: bool,
    /// 登录 key 回信的冻结收件人不再匹配。
    pub recipient_renamed: bool,
    /// 恢复 owner 是否允许重排关联动作。无关联时为真。
    pub recovery_retry_eligible: bool,
}

/// 阻断项闭集；顺序即判定优先级，调用方取第一项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryAdminGuardBlocker {
    /// 投递动作由另一个 owner 推进：结算退件 Take 归退件案件，寄售处置 Take/Return 归寄售专用
    /// 起写链。结果未知只认系统可验证证明，不允许人工释放、重试或声明成功/失败；同一邮件已
    /// 交接给结算退件 Take 后，原领取或退回意图也不能被重新激活。
    SettlementReturnActionOwned,
    /// 结算义务已不可执行（成交已撤销）、投递已不是当前派发，或恢复流程已接管该义务被退回的
    /// 邮件，禁止重新激活。
    SettlementIneligible,
    /// 已绑定资金责任，禁止人工释放/重试/对账改写。
    FundedDelivery,
    /// 当前持久行不能作为释放、重排或对账命令的快照。
    CommandSnapshotUnreadable,
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
    /// 重新排队不能把账号当前的登录 key 寄往旧角色。
    RecipientRenamed,
    /// 本 fence 已有解除告警的处置事实。
    AlreadyDisposed,
    /// 历史失败仍含资金、资产或履约责任，或当前并非失败态。
    NotAcknowledgeable,
    /// 父恢复或关联动作的状态、阻断归属或证明不允许重排。
    RecoveryRetryIneligible,
    /// 本投递关联的履约行尚不允许该人工迁移。
    SettlementFulfillmentState,
    /// 当前投递缺少可支撑普通对账终态的发送围栏。
    MissingSendingFence,
}

impl DeliveryAdminGuardBlocker {
    /// 映射回提交端的稳定 `DeliveryAdminError` 分类。
    pub const fn error(self) -> DeliveryAdminError {
        match self {
            Self::SettlementIneligible => DeliveryAdminError::SettlementIneligible,
            Self::RecipientRenamed => DeliveryAdminError::RecipientRenamed,
            Self::CommandSnapshotUnreadable => DeliveryAdminError::CommandSnapshotUnreadable,
            Self::SettlementReturnActionOwned
            | Self::FundedDelivery
            | Self::RetiredDelivery
            | Self::ReconcileSucceededForSend
            | Self::QuarantinedMailActionNotAllowed
            | Self::MissingReconcileReference
            | Self::AlreadyDisposed
            | Self::NotAcknowledgeable => DeliveryAdminError::Invalid,
            Self::SentSighted => DeliveryAdminError::SentSighted,
            Self::StaleFencingVersion
            | Self::DeliveryNotEligible
            | Self::StatusMismatch
            | Self::RecoveryRetryIneligible
            | Self::SettlementFulfillmentState
            | Self::MissingSendingFence => DeliveryAdminError::Stale,
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

/// 会重新激活投递的动作：需要读取 lot 发送资格与结算义务资格。
const fn checks_lot_eligibility(action: DeliveryAdminAction) -> bool {
    matches!(
        action,
        DeliveryAdminAction::Release
            | DeliveryAdminAction::RetryFailed
            | DeliveryAdminAction::ReconcileRetry
    )
}

/// 是否需要在 Sent 目击时失败关闭的动作。
fn checks_sent_sighting(action: DeliveryAdminAction, status: &str) -> bool {
    matches!(
        (action, status),
        (DeliveryAdminAction::RetryFailed, "failed")
            | (DeliveryAdminAction::ReconcileRetry | DeliveryAdminAction::ReconcileFailed, _)
    )
}

impl DeliveryAdminGuardSnapshot {
    /// 预览使用当前 fence，并假定操作员会填写命令要求的字段；提交时仍复核实际输入和持久事实。
    pub fn projected_blocker(
        &self,
        action: DeliveryAdminAction,
    ) -> Option<DeliveryAdminGuardBlocker> {
        Self {
            action,
            expected_fencing_version: self.fencing_version,
            has_reconcile_reference: true,
            ..self.clone()
        }
        .evaluate()
        .blockers
        .into_iter()
        .next()
    }

    /// 计算唯一判定；阻断项按判定优先级排列。
    pub fn evaluate(&self) -> DeliveryAdminGuardDecision {
        let transition = self.action.transition();
        let mut blockers = Vec::new();
        // 提交先锁资金与结算资格，再在投递锁内判断其余事实，预览按同一优先级给原因。
        if checks_lot_eligibility(self.action) && self.settlement_eligible == Some(false) {
            blockers.push(DeliveryAdminGuardBlocker::SettlementIneligible);
        }
        if self.funded_delivery {
            blockers.push(DeliveryAdminGuardBlocker::FundedDelivery);
        }
        if !self.command_snapshot_readable {
            blockers.push(DeliveryAdminGuardBlocker::CommandSnapshotUnreadable);
        }
        // 寄售处置投递只能由寄售专用链按 `mail_disposition_write_starts` 的一次性起写许可执行；
        // 通用 Admin 动作的新命令一律拒绝（不变量 4：结果未知只转人工核对，绝不自动重发；
        // 起写许可只发一次）。已提交命令的重放由调用方先查账本，不受此阻断影响；详情读取也不经
        // 本判定。这里复用既有映射为 `Invalid` 的“另一 owner 持有动作”阻断，避免新增 HTTP
        // 原因码；Admin 对寄售处置的核对出口在寄售自己的页面。
        if self.origin_kind == "settlement_return_action"
            || self.origin_kind == "consignment_disposition"
            || (checks_lot_eligibility(self.action) && self.mail_handed_over)
        {
            blockers.push(DeliveryAdminGuardBlocker::SettlementReturnActionOwned);
        }
        if self.retired {
            blockers.push(DeliveryAdminGuardBlocker::RetiredDelivery);
        }
        if self.fencing_version <= 0 || self.fencing_version != self.expected_fencing_version {
            blockers.push(DeliveryAdminGuardBlocker::StaleFencingVersion);
        }
        if checks_sent_sighting(self.action, &self.status)
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
        if checks_lot_eligibility(self.action) && self.recipient_renamed {
            blockers.push(DeliveryAdminGuardBlocker::RecipientRenamed);
        }
        if matches!(
            self.action,
            DeliveryAdminAction::RetryFailed | DeliveryAdminAction::ReconcileRetry
        ) && !self.recovery_retry_eligible
        {
            blockers.push(DeliveryAdminGuardBlocker::RecoveryRetryIneligible);
        }
        if self.settlement_fulfillment_state.as_deref().is_some_and(|state| match self.action {
            DeliveryAdminAction::Release => false,
            DeliveryAdminAction::RetryFailed | DeliveryAdminAction::ReconcileRetry => {
                !matches!(state, "failed" | "manual_reconcile")
            }
            DeliveryAdminAction::ReconcileSucceeded | DeliveryAdminAction::ReconcileFailed => {
                state != "manual_reconcile"
            }
        }) {
            blockers.push(DeliveryAdminGuardBlocker::SettlementFulfillmentState);
        }
        let disposition_conflict = self.reason_code.as_deref() == Some("mail_disposition_conflict");
        if disposition_conflict && self.action != DeliveryAdminAction::ReconcileFailed {
            // 隔离发生在发送前；旧 payload 没有重新发送或伪装外部成功的授权。
            blockers.push(DeliveryAdminGuardBlocker::QuarantinedMailActionNotAllowed);
        }
        if matches!(
            self.action,
            DeliveryAdminAction::ReconcileSucceeded | DeliveryAdminAction::ReconcileFailed
        ) && !self.has_sending_fence
            && !disposition_conflict
        {
            // 普通对账结果须保留真实的发送时间链；已知邮件处置冲突另有兼容终态。
            blockers.push(DeliveryAdminGuardBlocker::MissingSendingFence);
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
                    if disposition_conflict {
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

#[cfg(test)]
mod tests {
    use crate::ports::delivery::admin::{DeliveryAdminAction, DeliveryAdminError};
    use crate::ports::delivery::admin_guard::{
        DeliveryAdminGuardBlocker, DeliveryAdminGuardSnapshot,
    };

    fn consignment_snapshot(action: DeliveryAdminAction) -> DeliveryAdminGuardSnapshot {
        DeliveryAdminGuardSnapshot {
            action,
            expected_fencing_version: 1,
            fencing_version: 1,
            status: action.transition().expected_status.to_owned(),
            reason_code: None,
            has_sending_fence: true,
            origin_kind: "consignment_disposition".to_owned(),
            payload_operation: Some("take".to_owned()),
            sent_reference_sighted: false,
            funded_delivery: false,
            command_snapshot_readable: true,
            retired: false,
            lot_eligible: Some(true),
            settlement_eligible: None,
            settlement_fulfillment_state: None,
            has_reconcile_reference: true,
            mail_handed_over: false,
            recipient_renamed: false,
            recovery_retry_eligible: true,
        }
    }

    #[test]
    fn consignment_disposition_blocks_every_delivery_admin_action() {
        for action in [
            DeliveryAdminAction::Release,
            DeliveryAdminAction::RetryFailed,
            DeliveryAdminAction::ReconcileRetry,
            DeliveryAdminAction::ReconcileSucceeded,
            DeliveryAdminAction::ReconcileFailed,
        ] {
            let decision = consignment_snapshot(action).evaluate();
            assert_eq!(
                decision.blockers,
                vec![DeliveryAdminGuardBlocker::SettlementReturnActionOwned],
                "动作 {action:?} 必须被寄售来源阻断"
            );
            assert_eq!(decision.blockers[0].error(), DeliveryAdminError::Invalid);
        }
    }
}
