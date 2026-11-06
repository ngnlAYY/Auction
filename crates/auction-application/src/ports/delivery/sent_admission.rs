//! 人工 Sent 证明的准入规则：两种互斥准入（发送结果未知、Admin 对账失败后更正）与
//! 冻结身份/时间窗/资金占用检查。
//!
//! 存储层按固定锁序读取并投影成本结构；本模块只做纯判定，不访问数据库、不写日志。
//! 阻断项文案是 CLI 与运维契约，不能被静默改写。

use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

use crate::ports::delivery::{DeliverySendExpectation, ManualSentEvidence};

/// 冻结载荷的装备 secret 引用。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SentSecretRef {
    pub kind: String,
    pub id: Uuid,
}

/// 冻结载荷变量；未列出的键不参与判定，形状由投递创建侧与数据库约束固定。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SentPayloadVariables {
    #[serde(default)]
    pub reference: Option<Uuid>,
    #[serde(default)]
    pub amount: Option<String>,
    #[serde(default)]
    pub item_name: Option<String>,
}

/// 冻结投递载荷。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SentPayload {
    pub schema_version: u16,
    pub operation: String,
    pub world: String,
    pub template: String,
    pub variables: SentPayloadVariables,
    pub credits: i64,
    pub cod: i64,
    pub equipment_secret_refs: Vec<SentSecretRef>,
}

/// 投递结果元数据中与准入相关的键。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SentResultMetadata {
    pub write_started: Option<bool>,
    pub retired: Option<bool>,
    pub resolution: Option<String>,
}

/// 待确认投递的冻结事实。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SentDelivery {
    pub id: Uuid,
    pub status: String,
    pub failure_code: Option<String>,
    pub fencing_version: i64,
    pub origin_kind: String,
    pub external_reference: Option<String>,
    pub lease_token: Option<String>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub recipient: String,
    pub command_id: Uuid,
    pub request_digest: Option<String>,
    pub activation_command_id: Option<Uuid>,
    pub activation_request_digest: Option<String>,
    pub sending_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub activated_at: Option<DateTime<Utc>>,
    pub result_metadata: SentResultMetadata,
    pub payload: SentPayload,
}

/// 原结算义务的冻结身份与金额。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SentObligation {
    pub id: Uuid,
    pub lot_id: Uuid,
    pub auction_id: Uuid,
    pub equipment_id: Uuid,
    pub world_id: String,
    pub obligation_kind: String,
    pub business_state: String,
    pub amount: i64,
    pub carrier_credit: i32,
    pub recipient_user_id: Option<String>,
    pub recipient_game_identity: Option<String>,
    pub prerequisite_obligation_id: Option<Uuid>,
}

/// 原义务的履约状态；两种准入只接受各自的状态组合。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SentFulfillment {
    pub execution_state: String,
    pub last_error_code: Option<String>,
    pub delivery_id: Option<Uuid>,
    pub delivery_operation: String,
    pub dispatch_world: String,
    pub claim_token: Option<String>,
    pub claim_expires_at: Option<DateTime<Utc>>,
    pub proof_kind: Option<String>,
    pub proof_reference: Option<String>,
    pub proof_digest: Option<String>,
    pub proof_observed_at: Option<DateTime<Utc>>,
    pub prepare_command_id: Option<Uuid>,
    pub prepare_request_digest: Option<String>,
    pub activate_command_id: Option<Uuid>,
    pub activate_request_digest: Option<String>,
}

/// 恢复 case 的并存处置。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SentRecovery {
    pub stage: Option<String>,
    pub resolution: Option<String>,
    pub blocking_code: Option<String>,
    pub manual_reconcile_delivery_id: Option<Uuid>,
    pub claim_token: Option<String>,
}

/// Admin 失败对账命令的结果 fence；只用于确认“失败结论之后投递再无变化”。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SentAdminFailure {
    pub command_id: Uuid,
    pub command_kind: String,
    pub status: Option<String>,
    pub fencing_version: Option<i64>,
    pub failure_code: Option<String>,
}

/// 卖方现金的未知占用及其原始收款 lot。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SentFundingAllocation {
    pub id: Uuid,
    pub uid: String,
    pub lot_id: Uuid,
    pub obligation_id: Uuid,
    pub delivery_id: Uuid,
    pub purpose: String,
    pub state: String,
    pub amount: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SentFundingLot {
    pub lot_id: Uuid,
    pub uid: String,
    pub source_kind: String,
    pub disposition: String,
    pub payment_obligation_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SentLot {
    pub auction_id: Uuid,
    pub equipment_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SentEquipment {
    pub world_id: String,
    pub external_equipment_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SentSettlement {
    pub buyer_amount: i64,
    pub seller_amount: i64,
}

/// 冻结凭据的当前状态与版本；只保留独占判定需要的字段。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SentCredential {
    pub id: Option<Uuid>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub scope: Option<String>,
    pub version: Option<i64>,
}

/// 资金账户的最小投影。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SentFundingAccount {
    pub uid: String,
    pub status: String,
}

/// 已按锁序读取的人工 Sent 准入快照。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SentAdmissionSnapshot {
    pub delivery: SentDelivery,
    pub obligation: Option<SentObligation>,
    pub fulfillment: Option<SentFulfillment>,
    pub recovery: Option<SentRecovery>,
    pub admin_failure: Option<SentAdminFailure>,
    pub funding_allocation: Option<SentFundingAllocation>,
    pub funding_lot: Option<SentFundingLot>,
    pub lot: Option<SentLot>,
    pub equipment: Option<SentEquipment>,
    pub settlement: Option<SentSettlement>,
    pub credential: Option<SentCredential>,
    pub accounts: Vec<SentFundingAccount>,
    pub player_name: Option<String>,
    pub cash_reservation: bool,
    pub prior_unbound: bool,
    pub conflict: bool,
    pub sending_fencing_version: i64,
    /// 由 `delivery_current_send_attempt` 冻结的期待；缺失时失败关闭。
    pub expectation: Option<DeliverySendExpectation>,
}

/// 准入阻断项闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SentAdmissionBlocker {
    UnsupportedDeliveryScope,
    RequiresUnclaimedWriteUnknownOrAdminFailure,
    FulfillmentIdentityOrClaimConflict,
    ConflictingRecoveryDisposition,
    ExistingBindingRetirementOrConflictingAttempt,
    RequiresUniqueReadySenderFundingAccount,
    SenderPlayerNameMismatch,
    FrozenCredentialRetiredOrChanged,
    SentMinuteOutsideFrozenWindow,
    RequiresExactUnknownSellerReservation,
    FrozenIdentityBodyMoneyOrAssetsMismatch,
}

impl SentAdmissionBlocker {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedDeliveryScope => "unsupported_delivery_scope",
            Self::RequiresUnclaimedWriteUnknownOrAdminFailure => {
                "requires_unclaimed_write_unknown_or_admin_failure"
            }
            Self::FulfillmentIdentityOrClaimConflict => "fulfillment_identity_or_claim_conflict",
            Self::ConflictingRecoveryDisposition => "conflicting_recovery_disposition",
            Self::ExistingBindingRetirementOrConflictingAttempt => {
                "existing_binding_retirement_or_conflicting_attempt"
            }
            Self::RequiresUniqueReadySenderFundingAccount => {
                "requires_unique_ready_sender_funding_account"
            }
            Self::SenderPlayerNameMismatch => "sender_player_name_mismatch",
            Self::FrozenCredentialRetiredOrChanged => "frozen_credential_retired_or_changed",
            Self::SentMinuteOutsideFrozenWindow => "sent_minute_outside_frozen_window",
            Self::RequiresExactUnknownSellerReservation => {
                "requires_exact_unknown_seller_reservation"
            }
            Self::FrozenIdentityBodyMoneyOrAssetsMismatch => {
                "frozen_identity_body_money_or_assets_mismatch"
            }
        }
    }
}

/// 准入结论；预览与提交事务共用同一判定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentAdmission {
    pub eligible: bool,
    pub blockers: Vec<SentAdmissionBlocker>,
    pub expected_fencing_version: i64,
    pub requires_attempt_history_acceptance: bool,
    pub corrects_admin_failure: bool,
}

fn check(blockers: &mut Vec<SentAdmissionBlocker>, condition: bool, blocker: SentAdmissionBlocker) {
    if !condition {
        blockers.push(blocker);
    }
}

impl SentAdmissionSnapshot {
    /// 计算准入结论；同一快照、证据与事务读取时刻的结论唯一。
    pub fn evaluate(&self, evidence: &ManualSentEvidence, now: DateTime<Utc>) -> SentAdmission {
        let delivery = &self.delivery;
        let metadata = &delivery.result_metadata;
        // 准入一：发送结果未知且已开始写出。
        let write_unknown = delivery.status == "manual_reconcile"
            && delivery.failure_code.as_deref() == Some("write_unknown")
            && metadata.write_started == Some(true);
        // 准入二：Admin 已把该投递对账为失败，且失败命令的结果 fence 仍等于当前投递行。
        let admin_failed = delivery.status == "failed"
            && metadata.resolution.as_deref() == Some("admin_reconciled_failed")
            && delivery.failure_code.is_some()
            && self.admin_failure.as_ref().is_some_and(|failure| {
                failure.command_kind == "reconcile"
                    && failure.status.as_deref() == Some("failed")
                    && failure.fencing_version == Some(delivery.fencing_version)
                    && failure.failure_code == delivery.failure_code
            });
        // 被判失败的邮件可能早于当前冻结尝试；早于当前尝试即须显式接受旧尝试关联。
        let requires_attempt_history_acceptance = self.prior_unbound
            || (admin_failed
                && delivery.sending_at.is_some_and(|start| {
                    evidence
                        .sent_minute
                        .checked_add_signed(chrono::Duration::minutes(1))
                        .is_none_or(|upper| upper <= start)
                }));
        let mut blockers = Vec::new();
        check(
            &mut blockers,
            delivery.origin_kind == "settlement"
                && delivery.payload.world == "main"
                && delivery.payload.operation == "send"
                && self.obligation.as_ref().is_some_and(|obligation| {
                    obligation.world_id == "main"
                        && matches!(
                            obligation.obligation_kind.as_str(),
                            "winner_equipment_cod"
                                | "unsold_return"
                                | "withdrawn_return"
                                | "seller_payout"
                        )
                }),
            SentAdmissionBlocker::UnsupportedDeliveryScope,
        );
        check(
            &mut blockers,
            (write_unknown || admin_failed)
                && delivery.external_reference.is_none()
                && delivery.lease_token.is_none()
                && delivery.lease_expires_at.is_none()
                && metadata.retired != Some(true),
            SentAdmissionBlocker::RequiresUnclaimedWriteUnknownOrAdminFailure,
        );
        let fulfillment_state_matches = self.fulfillment.as_ref().is_some_and(|fulfillment| {
            if admin_failed {
                fulfillment.execution_state == "failed"
                    && fulfillment.last_error_code == delivery.failure_code
            } else {
                fulfillment.execution_state == "manual_reconcile"
            }
        });
        check(
            &mut blockers,
            fulfillment_state_matches
                && self.fulfillment.as_ref().is_some_and(|fulfillment| {
                    fulfillment.delivery_id == Some(delivery.id)
                        && fulfillment.delivery_operation == "send"
                        && fulfillment.dispatch_world == "main"
                        && fulfillment.claim_token.is_none()
                        && fulfillment.claim_expires_at.is_none()
                        && fulfillment.proof_kind.is_none()
                        && fulfillment.proof_reference.is_none()
                        && fulfillment.proof_digest.is_none()
                        && fulfillment.proof_observed_at.is_none()
                        && Some(delivery.command_id) == fulfillment.prepare_command_id
                        && delivery.request_digest.is_some()
                        && delivery.request_digest == fulfillment.prepare_request_digest
                        && delivery.activation_command_id == fulfillment.activate_command_id
                        && delivery.activation_request_digest == fulfillment.activate_request_digest
                })
                && self
                    .obligation
                    .as_ref()
                    .is_some_and(|obligation| obligation.business_state == "ready"),
            SentAdmissionBlocker::FulfillmentIdentityOrClaimConflict,
        );
        // 逾期只表示截止已过并发过提醒，不改变付款或资产事实；提醒以外的恢复动作仍冲突。
        check(
            &mut blockers,
            self.recovery.as_ref().is_none_or(|recovery| {
                matches!(recovery.stage.as_deref(), Some("awaiting_payment" | "payment_overdue"))
                    && recovery.resolution.is_none()
                    && recovery.blocking_code.is_none()
                    && recovery.manual_reconcile_delivery_id.is_none()
                    && recovery.claim_token.is_none()
            }),
            SentAdmissionBlocker::ConflictingRecoveryDisposition,
        );
        check(
            &mut blockers,
            !self.conflict,
            SentAdmissionBlocker::ExistingBindingRetirementOrConflictingAttempt,
        );
        check(
            &mut blockers,
            self.accounts.len() == 1
                && self.accounts.first().is_some_and(|account| {
                    account.uid == evidence.sender_uid && account.status == "ready"
                }),
            SentAdmissionBlocker::RequiresUniqueReadySenderFundingAccount,
        );
        check(
            &mut blockers,
            self.player_name.as_ref().is_none_or(|name| *name == evidence.sender_name),
            SentAdmissionBlocker::SenderPlayerNameMismatch,
        );
        check(
            &mut blockers,
            self.expectation.as_ref().is_some_and(|expected| {
                expected.credential.as_ref().is_some_and(|binding| {
                    self.credential.as_ref().is_some_and(|credential| {
                        credential.id == Some(binding.credential_id)
                            && credential.kind.as_deref() == Some("mail_cookie")
                            && credential.status.as_deref() == Some("active")
                            && credential.version == Some(binding.credential_version)
                            && matches!(
                                credential.scope.as_deref(),
                                Some("mooglemail:main" | "mooglemail:shared")
                            )
                    })
                })
            }),
            SentAdmissionBlocker::FrozenCredentialRetiredOrChanged,
        );
        // 结果未知只认当前尝试时窗；Admin 失败更正从投递激活所在分钟起至失败结论落定。
        let window_start = if admin_failed {
            delivery.activated_at.map(crate::ports::mail::sent_list_minute_start)
        } else {
            delivery.sending_at
        };
        check(
            &mut blockers,
            match (window_start, delivery.sending_at, delivery.completed_at) {
                (Some(start), Some(sending), Some(end)) => {
                    start <= sending
                        && sending <= end
                        && evidence.sent_minute <= end
                        && (if admin_failed {
                            evidence.sent_minute >= start
                        } else {
                            evidence
                                .sent_minute
                                .checked_add_signed(chrono::Duration::minutes(1))
                                .is_some_and(|upper| upper > start)
                        })
                        && evidence.observed_at >= end
                        && evidence.observed_at <= now
                }
                _ => false,
            },
            SentAdmissionBlocker::SentMinuteOutsideFrozenWindow,
        );
        if self.obligation.as_ref().is_some_and(|o| o.obligation_kind == "seller_payout") {
            // 只消除原始卖方支出的未知占用；正常税款等分配保留，不能因支持现金而豁免资金身份冲突。
            check(
                &mut blockers,
                self.seller_reservation_matches(evidence),
                SentAdmissionBlocker::RequiresExactUnknownSellerReservation,
            );
        }
        check(
            &mut blockers,
            self.frozen_identity_matches(evidence) && self.sending_fencing_version > 0,
            SentAdmissionBlocker::FrozenIdentityBodyMoneyOrAssetsMismatch,
        );
        SentAdmission {
            eligible: blockers.is_empty(),
            blockers,
            expected_fencing_version: delivery.fencing_version,
            requires_attempt_history_acceptance,
            corrects_admin_failure: admin_failed,
        }
    }

    fn seller_reservation_matches(&self, evidence: &ManualSentEvidence) -> bool {
        let (Some(obligation), Some(allocation), Some(funded_lot)) =
            (&self.obligation, &self.funding_allocation, &self.funding_lot)
        else {
            return false;
        };
        allocation.uid == evidence.sender_uid
            && allocation.lot_id == obligation.lot_id
            && allocation.obligation_id == obligation.id
            && allocation.delivery_id == self.delivery.id
            && allocation.purpose == "seller"
            && allocation.state == "unknown"
            && allocation.amount > 0
            && allocation.amount == obligation.amount
            && self.cash_reservation
            && funded_lot.lot_id == obligation.lot_id
            && funded_lot.uid == evidence.sender_uid
            && funded_lot.source_kind == "collected"
            && funded_lot.disposition == "seller"
            && obligation.prerequisite_obligation_id.is_some()
            && funded_lot.payment_obligation_id == obligation.prerequisite_obligation_id
    }

    /// 冻结期待必须与证据的收件人、主题、正文摘要、金额与装备逐一一致。
    fn frozen_identity_matches(&self, evidence: &ManualSentEvidence) -> bool {
        let Some(expected) = &self.expectation else {
            return false;
        };
        let item_name = self.delivery.payload.variables.item_name.as_deref().unwrap_or("");
        if !evidence.matches_frozen(expected, item_name)
            || expected.credential.is_none()
            || expected.world_id != self.delivery.payload.world
            || expected.recipient != self.delivery.recipient
            || expected.credits != self.delivery.payload.credits
            || expected.cod != self.delivery.payload.cod
        {
            return false;
        }
        let (Some(obligation), Some(lot), Some(equipment), Some(settlement)) =
            (&self.obligation, &self.lot, &self.equipment, &self.settlement)
        else {
            return false;
        };
        match obligation.obligation_kind.as_str() {
            "seller_payout" => {
                expected.credits > 0
                    && expected.cod == 0
                    && expected.equipment_ids.is_empty()
                    && obligation.amount == expected.credits
                    && obligation.carrier_credit == 0
                    && obligation.recipient_user_id.is_none()
                    && obligation.recipient_game_identity.as_ref() == Some(&expected.recipient)
                    && lot.auction_id == obligation.auction_id
                    && lot.equipment_id == obligation.equipment_id
                    && equipment.world_id == "main"
                    && settlement.seller_amount == obligation.amount
                    && self.delivery.payload
                        == SentPayload {
                            schema_version: 1,
                            operation: "send".to_owned(),
                            world: "main".to_owned(),
                            template: "settlement_seller_payout".to_owned(),
                            variables: SentPayloadVariables {
                                reference: Some(obligation.id),
                                amount: Some(expected.credits.to_string()),
                                item_name: None,
                            },
                            credits: expected.credits,
                            cod: 0,
                            equipment_secret_refs: Vec::new(),
                        }
            }
            kind @ ("winner_equipment_cod" | "unsold_return" | "withdrawn_return") => {
                let single_equipment = expected
                    .equipment_ids
                    .first()
                    .is_some_and(|id| equipment.external_equipment_id == *id);
                let secret_ref = self.delivery.payload.equipment_secret_refs.as_slice()
                    == [SentSecretRef {
                        kind: "auction_equipment_key".to_owned(),
                        id: obligation.equipment_id,
                    }];
                let template_matches = match kind {
                    "winner_equipment_cod" => {
                        self.delivery.payload.template == "settlement_winner_equipment_cod"
                            && obligation.amount == expected.cod
                    }
                    _ => {
                        self.delivery.payload.template == "settlement_equipment_handoff"
                            && expected.cod == 0
                    }
                };
                single_equipment && secret_ref && template_matches
            }
            _ => false,
        }
    }
}
