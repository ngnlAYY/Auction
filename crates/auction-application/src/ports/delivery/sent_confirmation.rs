//! 原生 Sent 人工观察的显式责任契约；它不是自动精确附件证明。
//!
//! 两种准入：结果未知（`manual_reconcile`/`write_unknown`）的发送，以及被 Admin 对账为失败、
//! 但原生 Sent 证明实际已寄出的发送。后者只追加更正事实，原失败命令与审计保持不变。

use auction_time::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::errors::ApplicationError;
use crate::ports::core::ApplicationFuture;
use crate::ports::core::RequestHash;
use crate::ports::delivery::sent_admission::check;
use crate::ports::delivery::{
    DeliverySendExpectation, SentAdmissionBlocker, SentAdmissionSnapshot,
};

impl SentAdmissionSnapshot {
    /// 失败更正可在读取详情前核对的条件与时窗。附件、回执和操作者身份仍由确认复核。
    pub fn failed_sent_observation_window(&self) -> Option<(Timestamp, Timestamp)> {
        if !self.current_admin_failure()
            || !self.delivery_blockers(true).is_empty()
            || !self.frozen_credential_matches()
            || !matches!(self.accounts.as_slice(), [account] if account.status == "ready")
            || self.sending_fencing_version <= 0
            || !self.expectation.as_ref().is_some_and(DeliverySendExpectation::validate)
        {
            return None;
        }
        self.sent_window(true)
    }

    pub(super) fn current_admin_failure(&self) -> bool {
        let delivery = &self.delivery;
        let metadata = &delivery.result_metadata;
        // 准入二：Admin 已把该投递对账为失败，且失败命令的结果 fence 仍等于当前投递行。
        delivery.status == "failed"
            && metadata.resolution.as_deref() == Some("admin_reconciled_failed")
            && delivery.failure_code.is_some()
            && self.admin_failure.as_ref().is_some_and(|failure| {
                failure.command_kind == "reconcile"
                    && failure.status.as_deref() == Some("failed")
                    && failure.fencing_version == Some(delivery.fencing_version)
                    && failure.failure_code == delivery.failure_code
            })
    }

    pub(super) fn delivery_blockers(&self, admin_failed: bool) -> Vec<SentAdmissionBlocker> {
        let delivery = &self.delivery;
        let metadata = &delivery.result_metadata;
        // 准入一：发送结果未知且已开始写出。
        let write_unknown = delivery.status == "manual_reconcile"
            && delivery.failure_code.as_deref() == Some("write_unknown")
            && metadata.write_started == Some(true);
        let mut blockers = Vec::new();
        check(
            &mut blockers,
            delivery.origin_kind == "settlement"
                && delivery
                    .payload
                    .as_ref()
                    .is_some_and(|payload| payload.world == "main" && payload.operation == "send")
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
        blockers
    }

    pub(super) fn frozen_credential_matches(&self) -> bool {
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
        })
    }

    pub(super) fn sent_window(&self, admin_failed: bool) -> Option<(Timestamp, Timestamp)> {
        let sending = self.delivery.sending_at?;
        let end = self.delivery.completed_at?;
        // 更正可追溯至投递激活所在分钟，结果未知仅检查当前尝试。
        let start = if admin_failed {
            crate::ports::mail::sent_list_minute_start(self.delivery.activated_at?)
        } else {
            sending
        };
        (start <= sending && sending <= end).then_some((start, end))
    }
}

/// 操作者保存的实际观察；已收取附件必须保持为空，不能填入期待 EID。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualSentEvidence {
    pub schema_version: u16,
    pub world_id: String,
    pub external_mail_id: String,
    pub sender_uid: String,
    pub sender_name: String,
    pub recipient: String,
    pub subject: String,
    pub sent_minute: Timestamp,
    pub observed_at: Timestamp,
    pub original_body_digest: [u8; 32],
    pub observed_body_digest: [u8; 32],
    pub remaining_credits: i64,
    pub cod_pending: i64,
    pub equipment_ids: Vec<String>,
    pub collected: bool,
    pub receipt: String,
}

impl ManualSentEvidence {
    /// Admin 以详情契约的微秒精度核对证据版本，Worker 的 JSON 可能保留纳秒。
    pub fn matches_observation_time(&self, observed_at: Option<Timestamp>) -> bool {
        observed_at.is_some_and(|recorded| {
            recorded.timestamp_micros() == self.observed_at.timestamp_micros()
        })
    }

    /// 仅验证规范形状；冻结身份、资金 UID 和分钟交集由最终事务核验。
    pub fn validate(&self) -> Result<(), ApplicationError> {
        if self.schema_version != 1
            || self.world_id != "main"
            || !auction_domain::valid_positive_decimal(&self.external_mail_id, 64)
            || !auction_domain::valid_positive_decimal(&self.sender_uid, 64)
            || !crate::ports::admin::valid_mail_player_name(&self.sender_name)
            || !crate::ports::admin::valid_mail_player_name(&self.recipient)
            || !auction_domain::valid_text(&self.subject, 512)
            || self.sent_minute.to_utc_datetime().second() != 0
            || self.sent_minute.timestamp_subsec_nanos() != 0
            || self.sent_minute.timestamp_nanos_opt().is_none()
            || self.observed_at.timestamp_nanos_opt().is_none()
            || self.observed_at < self.sent_minute
            || self.original_body_digest == [0; 32]
            || self.observed_body_digest == [0; 32]
            || !(0..=crate::ports::delivery::MAX_DELIVERY_COD).contains(&self.remaining_credits)
            || !(0..=crate::ports::delivery::MAX_DELIVERY_COD).contains(&self.cod_pending)
            || self.equipment_ids.len() > 1
            || self.equipment_ids.iter().any(|id| !auction_domain::valid_positive_decimal(id, 64))
            || self.receipt.len() > 2048
        {
            return Err(ApplicationError::FulfillmentDependencyMismatch);
        }
        Ok(())
    }

    /// 现金附件未领取时须原额仍在；已领取时只接受与冻结金额一致的原生回执，并由命令显式
    /// 接受领取责任。装备收取回执同样需命令显式接受缺失 EID 的责任。
    pub fn matches_frozen(&self, expected: &DeliverySendExpectation, item_name: &str) -> bool {
        if !expected.validate()
            || expected.world_id != self.world_id
            || expected.recipient != self.recipient
            || expected.subject != self.subject
            || expected.body_digest != self.original_body_digest
        {
            return false;
        }
        if expected.credits > 0 {
            if expected.cod != 0
                || !expected.equipment_ids.is_empty()
                || self.cod_pending != 0
                || !self.equipment_ids.is_empty()
            {
                return false;
            }
            if !self.collected {
                return self.receipt.is_empty()
                    && self.observed_body_digest == self.original_body_digest
                    && self.remaining_credits == expected.credits;
            }
            // 附件现金在发送时已离开账户，领取只改变收件人侧；回执金额必须与冻结金额逐字一致。
            return self.remaining_credits == 0
                && self.observed_body_digest != self.original_body_digest
                && self.receipt
                    == format!(
                        "Removed attachment: {} Credits",
                        grouped_credits(expected.credits)
                    );
        }
        if expected.credits != 0
            || expected.equipment_ids.len() != 1
            || !auction_domain::valid_text(item_name, 512)
        {
            return false;
        }
        if !self.collected {
            return self.receipt.is_empty()
                && self.observed_body_digest == self.original_body_digest
                && self.remaining_credits == expected.credits
                && self.cod_pending == expected.cod
                && self.equipment_ids == expected.equipment_ids;
        }
        let receipt = if expected.cod > 0 {
            format!(
                "CoD Paid: {} Credits\n\nRemoved attachment: {item_name}",
                grouped_credits(expected.cod)
            )
        } else {
            format!("Removed attachment: {item_name}")
        };
        self.remaining_credits == 0
            && self.cod_pending == 0
            && self.equipment_ids.is_empty()
            && self.observed_body_digest != self.original_body_digest
            && self.receipt == receipt
    }
}

fn grouped_credits(amount: i64) -> String {
    let digits = amount.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.bytes().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(char::from(digit));
    }
    grouped
}

/// 所选 Worker 观察的实见附件；未知和实见空数组含义不同。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct SentObservedAttachments {
    pub collected: Option<bool>,
    pub remaining_equipment_ids: Option<Vec<String>>,
    pub pre_collection_equipment_ids: Option<Vec<String>>,
}

impl SentObservedAttachments {
    pub(super) fn blocker(
        &self,
        evidence: &ManualSentEvidence,
        expected: &[String],
    ) -> Option<SentAdmissionBlocker> {
        let remaining = self.remaining_equipment_ids.as_deref();
        let before_collection = self.pre_collection_equipment_ids.as_deref();
        if expected.is_empty() || !evidence.collected || self.collected == Some(false) {
            let Some(remaining) = remaining else {
                return Some(SentAdmissionBlocker::MissingObservedEquipmentIds);
            };
            if remaining != expected || (!evidence.collected && remaining != evidence.equipment_ids)
            {
                return Some(SentAdmissionBlocker::ObservedEquipmentIdsMismatch);
            }
        }
        // 操作者声明已领取也不能豁免 Worker 已经实见的不符编号（不变量 6）。
        if before_collection.is_some_and(|ids| ids != expected)
            || remaining.is_some_and(|ids| !ids.is_empty() && ids != expected)
        {
            return Some(SentAdmissionBlocker::ObservedEquipmentIdsMismatch);
        }
        None
    }
}

/// 稳定命令绑定原观察、当前 fence 和两项独立人工责任。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SentConfirmationCommand {
    pub command_id: Uuid,
    pub delivery_id: Uuid,
    pub expected_fencing_version: i64,
    pub actor: String,
    /// 主机身份不改变原命令的幂等摘要。
    #[serde(skip)]
    pub operator_identity: Option<crate::ports::core::CliOperatorIdentity>,
    pub reason: String,
    pub evidence: ManualSentEvidence,
    pub accept_collected_assets: bool,
    pub accept_attempt_history: bool,
}

impl SentConfirmationCommand {
    pub fn validate(&self) -> Result<(), ApplicationError> {
        self.evidence.validate()?;
        if self.command_id.is_nil()
            || self.delivery_id.is_nil()
            || self.expected_fencing_version <= 0
            || !auction_domain::valid_text(&self.actor, 128)
            || !auction_domain::valid_text(&self.reason, 2048)
        {
            return Err(ApplicationError::FulfillmentDependencyMismatch);
        }
        Ok(())
    }

    pub fn request_digest(&self) -> Result<[u8; 32], ApplicationError> {
        RequestHash::from_request("delivery_sent_confirmation_v1", self.command_id, self)
            .map(RequestHash::as_bytes)
            .map_err(ApplicationError::from)
    }
}

/// 无副作用准入快照；必须由最终事务重新读取，不能把预览当作成功证明。
#[derive(Debug, Clone, Serialize)]
pub struct SentConfirmationPreview {
    pub eligible: bool,
    pub blockers: Vec<String>,
    pub delivery_id: Uuid,
    pub expected_fencing_version: i64,
    pub external_reference: String,
    pub requires_collected_asset_acceptance: bool,
    pub requires_attempt_history_acceptance: bool,
    /// 供 HTTP 的显式投影使用，保留 CLI 预览的既有字段。
    #[serde(skip_serializing)]
    pub prior_unbound_attempts: bool,
    #[serde(skip_serializing)]
    pub sent_before_current_attempt: bool,
    /// 目标是被 Admin 对账为失败的投递；确认即以 Sent 证据更正该人工失败结论。
    pub corrects_admin_failure: bool,
}

/// 原命令结果永久保存，同 ID 同参重放不重复追加事实。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SentConfirmationResult {
    pub command_id: Uuid,
    pub delivery_id: Uuid,
    pub external_reference: String,
    pub replayed: bool,
}

/// Admin 按证据引用选观察，CLI 按证据 MID 选观察；二者都以同一存储准入为准。
pub trait DeliverySentConfirmationPort: Send + Sync {
    fn preview_sent_confirmation<'a>(
        &'a self,
        delivery_id: Uuid,
        evidence: ManualSentEvidence,
        evidence_reference: Option<&'a str>,
    ) -> ApplicationFuture<'a, SentConfirmationPreview>;
    fn confirm_sent<'a>(
        &'a self,
        command: SentConfirmationCommand,
        evidence_reference: Option<&'a str>,
    ) -> ApplicationFuture<'a, SentConfirmationResult>;
}
