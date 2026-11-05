//! 装备损失人工处置的准入规则：只接受“主世界已结算 sold lot + 仅记账账户 +
//! 三个未动作义务 + 未发起的 COD 投递”这一种可证明的僵局，并给出闭集阻断项。
//!
//! 快照由存储层按固定锁序读取后按字段投影成结构体；本模块只做纯判定，
//! 不读取数据库、不写日志，也不构造第二套金额计算。

use serde::{Deserialize, Deserializer};
use uuid::Uuid;

use crate::{DomainError, FundingSplit, SettlementAmount};

/// 结算义务的持久化投影；只保留判定与提交 fence 需要的字段。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EquipmentLossObligation {
    pub id: Uuid,
    pub obligation_kind: String,
    pub business_state: String,
    pub amount: i64,
    pub prerequisite_obligation_id: Option<Uuid>,
    pub lot_id: Uuid,
    pub auction_id: Uuid,
    pub equipment_id: Uuid,
    pub world_id: String,
    pub recipient_user_id: Option<String>,
}

/// 履约状态投影；claim、证明与命令绑定都必须为空才算未动作。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EquipmentLossFulfillment {
    pub execution_state: String,
    pub state_version: i64,
    pub delivery_id: Option<Uuid>,
    pub prepare_command_id: Option<Uuid>,
    pub activate_command_id: Option<Uuid>,
    /// `bytea` 列在 `to_jsonb` 中是不可解释的文本；只做同一来源的相等比较。
    pub prepare_request_digest: Option<String>,
    pub activate_request_digest: Option<String>,
    pub claim_token: Option<String>,
    pub claimed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub claim_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub proof_kind: Option<String>,
    pub proof_reference: Option<String>,
    pub proof_digest: Option<String>,
    pub proof_observed_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// 恢复 case 投影；已出现任何处置、复核或阻断都必须拒绝。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EquipmentLossRecovery {
    pub id: Uuid,
    pub lot_id: Uuid,
    pub auction_id: Uuid,
    pub equipment_id: Uuid,
    pub payment_obligation_id: Uuid,
    pub winner_user_id: Option<String>,
    pub world_id: String,
    pub stage: String,
    pub state_version: i64,
    pub cod_return_count: i32,
    pub reminder_count: i32,
    pub resolution: Option<String>,
    pub equipment_obligation_id: Option<Uuid>,
    pub return_mail_id: Option<Uuid>,
    pub return_external_mail_id: Option<String>,
    pub custody_proof_kind: Option<String>,
    pub custody_proof_reference: Option<String>,
    pub custody_proof_digest: Option<String>,
    pub custody_proof_observed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub relisted_auction_id: Option<Uuid>,
    pub relisted_lot_id: Option<Uuid>,
    pub blocking_code: Option<String>,
    pub manual_reconcile_delivery_id: Option<Uuid>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// 只表达“该键存在且不是 JSON null”，内容不参与判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentValue;

impl<'de> Deserialize<'de> for PresentValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        serde::de::IgnoredAny::deserialize(deserializer)?;
        Ok(Self)
    }
}

/// 投递结果元数据；只认未写出的读取阶段，任何证明或已核验 Sent 都拒绝。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EquipmentLossResultMetadata {
    pub operation: Option<String>,
    pub stage: Option<String>,
    pub write_started: Option<bool>,
    pub proof: Option<PresentValue>,
    pub sent_verified: Option<PresentValue>,
}

/// 冻结载荷中的装备 secret 引用；只允许指向本义务装备。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EquipmentLossSecretRef {
    pub kind: String,
    pub id: Uuid,
}

/// 冻结投递载荷；未列出的键不参与该场景的判定。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EquipmentLossDeliveryPayload {
    pub schema_version: u16,
    pub operation: String,
    pub world: String,
    pub template: String,
    pub credits: i64,
    pub cod: i64,
    #[serde(default)]
    pub equipment_secret_refs: Vec<EquipmentLossSecretRef>,
}

/// 待处置投递的完整投影。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EquipmentLossDelivery {
    pub id: Uuid,
    pub status: String,
    pub attempt_count: i32,
    pub fencing_version: i64,
    pub failure_code: Option<String>,
    pub origin_kind: String,
    pub origin_id: String,
    pub command_id: Uuid,
    pub request_digest: Option<String>,
    pub activation_command_id: Option<Uuid>,
    pub activation_request_digest: Option<String>,
    pub external_reference: Option<String>,
    pub lease_token: Option<String>,
    pub lease_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub result_metadata: EquipmentLossResultMetadata,
    pub payload: EquipmentLossDeliveryPayload,
}

/// 资金账户投影；只保留唯一性与记账模式判定。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EquipmentLossFundingAccount {
    pub uid: String,
    pub status: String,
    pub accounting_mode: String,
}

/// 拍卖与 lot 的冻结事实。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EquipmentLossAuction {
    pub id: Uuid,
    pub world_id: String,
    pub status: String,
    pub settled_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EquipmentLossLot {
    pub id: Uuid,
    pub auction_id: Uuid,
    pub equipment_id: Uuid,
    pub status: String,
    pub leader_user_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EquipmentLossEquipment {
    pub id: Uuid,
    pub world_id: String,
    pub external_equipment_id: String,
}

/// 冻结结算金额；只用于装配 `FundingSplit`，不重新计算价格。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EquipmentLossSettlement {
    pub buyer_amount: i64,
    pub compensation_amount: i64,
    pub seller_amount: i64,
    pub tax_amount: i64,
}

/// 准入阻断项闭集；文案是既有运维契约，不能静默改写。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EquipmentLossBlocker {
    UniqueReadyRecordOnlyAccount,
    MainSettledSoldLot,
    FrozenObligationMismatch,
    OwnerIdentityMismatch,
    RecoveryAlreadyActed,
    FulfillmentAlreadyActedOrClaimed,
    OriginalAttemptNotProvenInventoryOnly,
    OriginalDeliveryPayloadMismatch,
    ExistingDispositionOrFunding,
    InvalidFrozenAmounts,
}

impl EquipmentLossBlocker {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UniqueReadyRecordOnlyAccount => "requires_unique_ready_record_only_account",
            Self::MainSettledSoldLot => "requires_main_settled_sold_lot",
            Self::FrozenObligationMismatch => "frozen_obligation_mismatch",
            Self::OwnerIdentityMismatch => "owner_identity_mismatch",
            Self::RecoveryAlreadyActed => "recovery_already_acted",
            Self::FulfillmentAlreadyActedOrClaimed => "fulfillment_already_acted_or_claimed",
            Self::OriginalAttemptNotProvenInventoryOnly => {
                "original_attempt_not_proven_inventory_only"
            }
            Self::OriginalDeliveryPayloadMismatch => "original_delivery_payload_mismatch",
            Self::ExistingDispositionOrFunding => "existing_disposition_or_funding",
            Self::InvalidFrozenAmounts => "invalid_frozen_amounts",
        }
    }
}

/// 已按锁序读取的装备损失准入快照。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EquipmentLossAdmission {
    pub auction: EquipmentLossAuction,
    pub lot: EquipmentLossLot,
    pub equipment: EquipmentLossEquipment,
    pub settlement: EquipmentLossSettlement,
    pub recovery: EquipmentLossRecovery,
    pub payment: EquipmentLossObligation,
    pub payment_fulfillment: EquipmentLossFulfillment,
    pub seller: EquipmentLossObligation,
    pub seller_fulfillment: EquipmentLossFulfillment,
    pub refund: EquipmentLossObligation,
    pub refund_fulfillment: EquipmentLossFulfillment,
    pub delivery: EquipmentLossDelivery,
    pub funding_accounts: Vec<EquipmentLossFundingAccount>,
    pub conflict: bool,
}

fn check(blockers: &mut Vec<EquipmentLossBlocker>, condition: bool, blocker: EquipmentLossBlocker) {
    if !condition {
        blockers.push(blocker);
    }
}

impl EquipmentLossAdmission {
    /// 冻结金额装配；调用方仍以命令参数和账户模式做最终 `equipment_loss_funding` 校验。
    pub fn funding_split(&self) -> Result<FundingSplit, DomainError> {
        let amount = |value: i64| SettlementAmount::new(value);
        Ok(FundingSplit {
            buyer: amount(self.settlement.buyer_amount)?,
            compensation: amount(self.settlement.compensation_amount)?,
            seller: amount(self.settlement.seller_amount)?,
            tax: amount(self.settlement.tax_amount)?,
        })
    }

    /// 按固定顺序返回全部阻断项；空集表示可以进入提交事务。
    pub fn evaluate(&self) -> Vec<EquipmentLossBlocker> {
        let mut blockers = Vec::new();
        let account = self.funding_accounts.first();
        check(
            &mut blockers,
            self.funding_accounts.len() == 1
                && account.is_some_and(|account| {
                    account.status == "ready" && account.accounting_mode == "record_only"
                }),
            EquipmentLossBlocker::UniqueReadyRecordOnlyAccount,
        );
        check(
            &mut blockers,
            self.auction.world_id == "main"
                && self.auction.status == "settled"
                && self.auction.settled_at.is_some()
                && self.lot.status == "sold",
            EquipmentLossBlocker::MainSettledSoldLot,
        );
        let (payment, seller, refund) = (&self.payment, &self.seller, &self.refund);
        check(
            &mut blockers,
            payment.obligation_kind == "winner_equipment_cod"
                && payment.business_state == "ready"
                && payment.amount == self.settlement.buyer_amount
                && seller.obligation_kind == "seller_payout"
                && seller.business_state == "blocked"
                && seller.amount == self.settlement.seller_amount
                && seller.prerequisite_obligation_id == Some(payment.id)
                && refund.obligation_kind == "buyer_refund"
                && refund.business_state == "contingent"
                && refund.amount == payment.amount
                && refund.prerequisite_obligation_id == Some(payment.id),
            EquipmentLossBlocker::FrozenObligationMismatch,
        );
        check(
            &mut blockers,
            [payment, seller, refund].iter().all(|obligation| {
                obligation.lot_id == self.lot.id
                    && obligation.auction_id == self.auction.id
                    && obligation.equipment_id == self.equipment.id
                    && obligation.world_id == "main"
            }) && self.recovery.equipment_id == self.equipment.id
                && self.recovery.winner_user_id == payment.recipient_user_id
                && payment.recipient_user_id == self.lot.leader_user_id
                && refund.recipient_user_id == payment.recipient_user_id
                && self.equipment.world_id == "main",
            EquipmentLossBlocker::OwnerIdentityMismatch,
        );
        let acted = [
            self.recovery.resolution.is_some(),
            self.recovery.equipment_obligation_id.is_some(),
            self.recovery.return_mail_id.is_some(),
            self.recovery.return_external_mail_id.is_some(),
            self.recovery.custody_proof_kind.is_some(),
            self.recovery.custody_proof_reference.is_some(),
            self.recovery.custody_proof_digest.is_some(),
            self.recovery.custody_proof_observed_at.is_some(),
            self.recovery.relisted_auction_id.is_some(),
            self.recovery.relisted_lot_id.is_some(),
            self.recovery.blocking_code.is_some(),
            self.recovery.manual_reconcile_delivery_id.is_some(),
            self.recovery.completed_at.is_some(),
        ];
        check(
            &mut blockers,
            self.recovery.world_id == "main"
                && self.recovery.stage == "awaiting_payment"
                && self.recovery.cod_return_count == 0
                && self.recovery.reminder_count == 0
                && !acted.iter().any(|acted| *acted),
            EquipmentLossBlocker::RecoveryAlreadyActed,
        );
        let pending = |fulfillment: &EquipmentLossFulfillment| {
            fulfillment.delivery_id.is_none()
                && fulfillment.prepare_command_id.is_none()
                && fulfillment.activate_command_id.is_none()
        };
        let unclaimed = |fulfillment: &EquipmentLossFulfillment| {
            fulfillment.claim_token.is_none()
                && fulfillment.claimed_at.is_none()
                && fulfillment.claim_expires_at.is_none()
                && fulfillment.proof_kind.is_none()
                && fulfillment.proof_reference.is_none()
                && fulfillment.proof_digest.is_none()
                && fulfillment.proof_observed_at.is_none()
        };
        check(
            &mut blockers,
            self.payment_fulfillment.execution_state == "manual_reconcile"
                && self.seller_fulfillment.execution_state == "pending"
                && self.refund_fulfillment.execution_state == "pending"
                && self.seller_fulfillment.state_version == 1
                && self.refund_fulfillment.state_version == 1
                && pending(&self.seller_fulfillment)
                && pending(&self.refund_fulfillment)
                && [&self.payment_fulfillment, &self.seller_fulfillment, &self.refund_fulfillment]
                    .iter()
                    .all(|fulfillment| unclaimed(fulfillment)),
            EquipmentLossBlocker::FulfillmentAlreadyActedOrClaimed,
        );
        let delivery = &self.delivery;
        let metadata = &delivery.result_metadata;
        check(
            &mut blockers,
            delivery.status == "manual_reconcile"
                && delivery.attempt_count == 1
                && delivery.fencing_version == 1
                && delivery.failure_code.as_deref() == Some("not_observed")
                && delivery.origin_kind == "settlement"
                && delivery.origin_id == payment.id.to_string()
                && Some(delivery.command_id) == self.payment_fulfillment.prepare_command_id
                && delivery.request_digest.is_some()
                && delivery.request_digest == self.payment_fulfillment.prepare_request_digest
                && delivery.activation_command_id == self.payment_fulfillment.activate_command_id
                && delivery.activation_request_digest
                    == self.payment_fulfillment.activate_request_digest
                && delivery.external_reference.is_none()
                && delivery.lease_token.is_none()
                && delivery.lease_expires_at.is_none()
                && metadata.operation.as_deref() == Some("inventory")
                && metadata.stage.as_deref() == Some("read")
                && metadata.write_started == Some(false)
                && metadata.proof.is_none()
                && metadata.sent_verified.is_none(),
            EquipmentLossBlocker::OriginalAttemptNotProvenInventoryOnly,
        );
        check(
            &mut blockers,
            delivery.payload.schema_version == 1
                && delivery.payload.operation == "send"
                && delivery.payload.world == "main"
                && delivery.payload.template == "settlement_winner_equipment_cod"
                && delivery.payload.cod == payment.amount
                && delivery.payload.credits == 0
                && delivery.payload.equipment_secret_refs
                    == [EquipmentLossSecretRef {
                        kind: "auction_equipment_key".to_owned(),
                        id: payment.equipment_id,
                    }],
            EquipmentLossBlocker::OriginalDeliveryPayloadMismatch,
        );
        check(&mut blockers, !self.conflict, EquipmentLossBlocker::ExistingDispositionOrFunding);
        self.finish(blockers)
    }

    fn finish(&self, mut blockers: Vec<EquipmentLossBlocker>) -> Vec<EquipmentLossBlocker> {
        let frozen_ok = self
            .funding_split()
            .and_then(|split| crate::equipment_loss_funding(split, true, true))
            .is_ok();
        check(&mut blockers, frozen_ok, EquipmentLossBlocker::InvalidFrozenAmounts);
        blockers
    }
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use super as parent;

    fn uuid(seed: u64) -> Uuid {
        Uuid::from_u64_pair(seed, seed)
    }

    fn obligation(
        id: u64,
        kind: &str,
        state: &str,
        amount: i64,
        prerequisite: Option<u64>,
    ) -> EquipmentLossObligation {
        EquipmentLossObligation {
            id: uuid(id),
            obligation_kind: kind.to_owned(),
            business_state: state.to_owned(),
            amount,
            prerequisite_obligation_id: prerequisite.map(uuid),
            lot_id: uuid(20),
            auction_id: uuid(21),
            equipment_id: uuid(22),
            world_id: "main".to_owned(),
            recipient_user_id: Some("1001".to_owned()),
        }
    }

    fn fulfillment(state: &str) -> EquipmentLossFulfillment {
        EquipmentLossFulfillment {
            execution_state: state.to_owned(),
            state_version: 1,
            delivery_id: None,
            prepare_command_id: None,
            activate_command_id: None,
            prepare_request_digest: None,
            activate_request_digest: None,
            claim_token: None,
            claimed_at: None,
            claim_expires_at: None,
            proof_kind: None,
            proof_reference: None,
            proof_digest: None,
            proof_observed_at: None,
        }
    }

    fn admission() -> EquipmentLossAdmission {
        EquipmentLossAdmission {
            auction: EquipmentLossAuction {
                id: uuid(21),
                world_id: "main".to_owned(),
                status: "settled".to_owned(),
                settled_at: Some(chrono::Utc::now()),
            },
            lot: EquipmentLossLot {
                id: uuid(20),
                auction_id: uuid(21),
                equipment_id: uuid(22),
                status: "sold".to_owned(),
                leader_user_id: Some("1001".to_owned()),
            },
            equipment: EquipmentLossEquipment {
                id: uuid(22),
                world_id: "main".to_owned(),
                external_equipment_id: "300753957".to_owned(),
            },
            settlement: EquipmentLossSettlement {
                buyer_amount: 5000,
                compensation_amount: 1000,
                seller_amount: 6000,
                tax_amount: 0,
            },
            recovery: EquipmentLossRecovery {
                id: uuid(23),
                lot_id: uuid(20),
                auction_id: uuid(21),
                equipment_id: uuid(22),
                payment_obligation_id: uuid(1),
                winner_user_id: Some("1001".to_owned()),
                world_id: "main".to_owned(),
                stage: "awaiting_payment".to_owned(),
                state_version: 3,
                cod_return_count: 0,
                reminder_count: 0,
                resolution: None,
                equipment_obligation_id: None,
                return_mail_id: None,
                return_external_mail_id: None,
                custody_proof_kind: None,
                custody_proof_reference: None,
                custody_proof_digest: None,
                custody_proof_observed_at: None,
                relisted_auction_id: None,
                relisted_lot_id: None,
                blocking_code: None,
                manual_reconcile_delivery_id: None,
                completed_at: None,
            },
            payment: obligation(1, "winner_equipment_cod", "ready", 5000, None),
            payment_fulfillment: {
                let mut fulfillment = fulfillment("manual_reconcile");
                fulfillment.delivery_id = Some(uuid(30));
                fulfillment.prepare_command_id = Some(uuid(31));
                fulfillment.activate_command_id = Some(uuid(32));
                fulfillment.prepare_request_digest = Some("\\x01".to_owned());
                fulfillment.activate_request_digest = Some("\\x02".to_owned());
                fulfillment
            },
            seller: obligation(2, "seller_payout", "blocked", 6000, Some(1)),
            seller_fulfillment: fulfillment("pending"),
            refund: obligation(3, "buyer_refund", "contingent", 5000, Some(1)),
            refund_fulfillment: fulfillment("pending"),
            delivery: EquipmentLossDelivery {
                id: uuid(30),
                status: "manual_reconcile".to_owned(),
                attempt_count: 1,
                fencing_version: 1,
                failure_code: Some("not_observed".to_owned()),
                origin_kind: "settlement".to_owned(),
                origin_id: uuid(1).to_string(),
                command_id: uuid(31),
                request_digest: Some("\\x01".to_owned()),
                activation_command_id: Some(uuid(32)),
                activation_request_digest: Some("\\x02".to_owned()),
                external_reference: None,
                lease_token: None,
                lease_expires_at: None,
                result_metadata: EquipmentLossResultMetadata {
                    operation: Some("inventory".to_owned()),
                    stage: Some("read".to_owned()),
                    write_started: Some(false),
                    proof: None,
                    sent_verified: None,
                },
                payload: EquipmentLossDeliveryPayload {
                    schema_version: 1,
                    operation: "send".to_owned(),
                    world: "main".to_owned(),
                    template: "settlement_winner_equipment_cod".to_owned(),
                    credits: 0,
                    cod: 5000,
                    equipment_secret_refs: vec![EquipmentLossSecretRef {
                        kind: "auction_equipment_key".to_owned(),
                        id: uuid(22),
                    }],
                },
            },
            funding_accounts: vec![EquipmentLossFundingAccount {
                uid: "9001".to_owned(),
                status: "ready".to_owned(),
                accounting_mode: "record_only".to_owned(),
            }],
            conflict: false,
        }
    }

    #[test]
    fn frozen_graph_admits_and_keeps_operator_split() {
        let admission = admission();
        assert_eq!(admission.evaluate(), Vec::new());
        let split = admission.funding_split().expect("冻结金额可装配");
        assert_eq!(split.buyer.value(), 5000);
        assert_eq!(split.compensation.value(), 1000);
        assert_eq!(split.seller.value(), 6000);
        assert_eq!(split.tax.value(), 0);
    }

    #[test]
    fn every_guard_rejects_independently() {
        let cases: Vec<(EquipmentLossAdmission, EquipmentLossBlocker)> = vec![
            (
                EquipmentLossAdmission {
                    funding_accounts: vec![EquipmentLossFundingAccount {
                        uid: "9001".to_owned(),
                        status: "ready".to_owned(),
                        accounting_mode: "ledger".to_owned(),
                    }],
                    ..admission()
                },
                EquipmentLossBlocker::UniqueReadyRecordOnlyAccount,
            ),
            (
                EquipmentLossAdmission {
                    auction: EquipmentLossAuction { settled_at: None, ..admission().auction },
                    ..admission()
                },
                EquipmentLossBlocker::MainSettledSoldLot,
            ),
            (
                EquipmentLossAdmission {
                    seller: obligation(2, "seller_payout", "ready", 6000, Some(1)),
                    ..admission()
                },
                EquipmentLossBlocker::FrozenObligationMismatch,
            ),
            (
                EquipmentLossAdmission {
                    lot: EquipmentLossLot {
                        leader_user_id: Some("2002".to_owned()),
                        ..admission().lot
                    },
                    ..admission()
                },
                EquipmentLossBlocker::OwnerIdentityMismatch,
            ),
            (
                EquipmentLossAdmission {
                    recovery: EquipmentLossRecovery { reminder_count: 1, ..admission().recovery },
                    ..admission()
                },
                EquipmentLossBlocker::RecoveryAlreadyActed,
            ),
            (
                EquipmentLossAdmission {
                    seller_fulfillment: {
                        let mut fulfillment = fulfillment("pending");
                        fulfillment.claim_token = Some("claim".to_owned());
                        fulfillment
                    },
                    ..admission()
                },
                EquipmentLossBlocker::FulfillmentAlreadyActedOrClaimed,
            ),
            (
                EquipmentLossAdmission {
                    delivery: EquipmentLossDelivery {
                        result_metadata: EquipmentLossResultMetadata {
                            write_started: Some(true),
                            ..admission().delivery.result_metadata
                        },
                        ..admission().delivery
                    },
                    ..admission()
                },
                EquipmentLossBlocker::OriginalAttemptNotProvenInventoryOnly,
            ),
            (
                EquipmentLossAdmission {
                    delivery: EquipmentLossDelivery {
                        payload: EquipmentLossDeliveryPayload {
                            cod: 1,
                            ..admission().delivery.payload
                        },
                        ..admission().delivery
                    },
                    ..admission()
                },
                EquipmentLossBlocker::OriginalDeliveryPayloadMismatch,
            ),
            (
                EquipmentLossAdmission { conflict: true, ..admission() },
                EquipmentLossBlocker::ExistingDispositionOrFunding,
            ),
            (
                EquipmentLossAdmission {
                    settlement: EquipmentLossSettlement {
                        seller_amount: 0,
                        ..admission().settlement
                    },
                    ..admission()
                },
                EquipmentLossBlocker::InvalidFrozenAmounts,
            ),
        ];
        for (admission, expected) in cases {
            let blockers = admission.evaluate();
            assert!(blockers.contains(&expected), "缺少阻断项 {:?}，实际 {:?}", expected, blockers);
        }
    }
}
