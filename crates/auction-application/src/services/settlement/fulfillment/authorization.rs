//! 履约阶段推进前的收件人、custody 与余额授权。

use std::time::Instant;

use auction_domain::SettlementObligationKind;
use chrono::Duration;

use super::evidence::validate_balance_evidence;
use super::intent::build_delivery_intent;
use super::model::SettlementDeliveryIntent;
use super::validation::repository_error;
use crate::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, SafeErrorKind, diagnostic_id, record_error,
};
use crate::{
    ApplicationError, Clock, FundingPolicy, InternalEquipmentTarget, InternalEvidencePurpose,
    InternalEvidenceRequest, PreparedSettlementInternalEvidence, SettlementFulfillmentClaim,
    SettlementFulfillmentCustodyReader, SettlementFulfillmentEvidenceReader,
    SettlementFulfillmentOperation, SettlementFulfillmentPhaseResult,
    SettlementFulfillmentRecipientReader, SettlementFulfillmentState, SettlementFulfillmentStore,
};

/// dependency 授权要么返回可继续使用的事实，要么已经持久化等待/人工态。
pub(super) enum AuthorizationResult<T> {
    Authorized(T),
    Deferred { obligation_id: auction_domain::SettlementId, failure_code: &'static str },
    ManualReconcile { obligation_id: auction_domain::SettlementId, failure_code: &'static str },
}

impl<T> AuthorizationResult<T> {
    /// 授权成功时取出事实；已持久化的等待/人工态转换为任一阶段的同名结果。
    pub(super) fn into_phase<R>(self) -> Result<T, SettlementFulfillmentPhaseResult<R>> {
        match self {
            Self::Authorized(value) => Ok(value),
            Self::Deferred { obligation_id, failure_code } => {
                Err(SettlementFulfillmentPhaseResult::Deferred { obligation_id, failure_code })
            }
            Self::ManualReconcile { obligation_id, failure_code } => {
                Err(SettlementFulfillmentPhaseResult::ManualReconcile {
                    obligation_id,
                    failure_code,
                })
            }
        }
    }
}

/// pending/prepared 每次推进都复核收件人和退款 custody；现金观察遵守持久资金策略。
// 这些依赖仍由应用用例显式传入；余额开关仅决定读侧观察，发送事务会重新核验策略。
#[allow(clippy::too_many_arguments)]
pub(super) async fn authorize_pending_or_prepared(
    store: &dyn SettlementFulfillmentStore,
    recipients: &dyn SettlementFulfillmentRecipientReader,
    custody: &dyn SettlementFulfillmentCustodyReader,
    evidence: &dyn SettlementFulfillmentEvidenceReader,
    clock: &dyn Clock,
    claim: SettlementFulfillmentClaim,
    retry_delay: Duration,
    policy: &FundingPolicy,
) -> Result<
    AuthorizationResult<(
        SettlementFulfillmentClaim,
        SettlementDeliveryIntent,
        Option<PreparedSettlementInternalEvidence>,
    )>,
    ApplicationError,
> {
    // 已冻结的旧 Return 不能改写成新 Send，也不能凭原邮件动作继续推进装备返还。
    if claim.operation == SettlementFulfillmentOperation::Return {
        return manual(store, claim, "legacy_return_requires_review").await;
    }
    let claim = match resolve_recipient(
        store,
        recipients,
        claim,
        retry_delay,
        true,
        "recipient_unavailable",
    )
    .await?
    {
        AuthorizationResult::Authorized(claim) => claim,
        AuthorizationResult::Deferred { obligation_id, failure_code } => {
            return Ok(AuthorizationResult::Deferred { obligation_id, failure_code });
        }
        AuthorizationResult::ManualReconcile { obligation_id, failure_code } => {
            return Ok(AuthorizationResult::ManualReconcile { obligation_id, failure_code });
        }
    };
    let intent = match build_delivery_intent(&claim) {
        Ok(intent) => intent,
        Err(_) => {
            return manual(store, claim, "invalid_fulfillment_fact").await;
        }
    };

    if claim.kind == SettlementObligationKind::BuyerRefund {
        let Some(budget) = (claim.lease_until - clock.now())
            .to_std()
            .ok()
            .map(|budget| budget.min(std::time::Duration::from_secs(30)))
            .filter(|budget| *budget > std::time::Duration::from_secs(1))
        else {
            record_error(
                DiagnosticLevel::Warn,
                SafeErrorKind::EVIDENCE_EXPIRED,
                "fulfillment_authorization",
                "check_custody_budget",
                DiagnosticContext {
                    obligation_id: diagnostic_id(&claim.obligation_id),
                    world: Some(claim.world),
                    defer_reason: Some("evidence_unavailable"),
                    ..Default::default()
                },
            );
            return defer(
                store,
                claim,
                retry_delay,
                "evidence_unavailable",
                "装备 custody 证明预算不足",
            )
            .await;
        };
        let operation_id = match claim.state {
            SettlementFulfillmentState::Pending => {
                super::intent::stable_prepare_command_id(claim.obligation_id)
            }
            SettlementFulfillmentState::Prepared => {
                super::intent::stable_activate_command_id(claim.obligation_id)
            }
            _ => return Err(repository_error("退款 custody 读取阶段无效")),
        }
        .as_uuid();
        let purpose = InternalEvidencePurpose::SettlementClaim {
            obligation_id: claim.obligation_id.as_uuid(),
            claim_token: claim.claim_token,
            claim_fencing: claim.claim_fencing,
            state_version: claim.state_version,
        };
        let request = InternalEvidenceRequest {
            operation_id,
            purpose: purpose.clone(),
            world: claim.world,
            equipment: Some(InternalEquipmentTarget {
                equipment_id: claim.equipment_id,
                external_equipment_id: claim.external_equipment_id.clone(),
                world: claim.equipment_world,
            }),
            observe_main_cash: policy.observes_cash(),
            deadline: Instant::now() + budget,
        };
        let prepared = match custody.read_custody(request).await {
            Ok(prepared) => prepared,
            Err(error) => {
                record_error(
                    DiagnosticLevel::Warn,
                    error.diagnostic_kind(),
                    "fulfillment_authorization",
                    "read_custody",
                    DiagnosticContext {
                        obligation_id: diagnostic_id(&claim.obligation_id),
                        world: Some(claim.world),
                        ..Default::default()
                    },
                );

                return defer(
                    store,
                    claim,
                    retry_delay,
                    "evidence_unavailable",
                    "同账号 Armory 正向库存证明暂不可用",
                )
                .await;
            }
        };
        let snapshot = prepared.snapshot();
        if snapshot.operation_id != operation_id
            || snapshot.purpose != purpose
            || snapshot.identity.world != claim.world
            || !policy.matches_identity(&snapshot.identity.authenticated_uid)
            || snapshot.equipment.as_ref().is_none_or(|equipment| {
                equipment.equipment_id != claim.equipment_id
                    || equipment.external_equipment_id != claim.external_equipment_id
                    || equipment.binding.world != claim.equipment_world
                    || !snapshot.identity.same_account(&equipment.binding)
            })
        {
            return manual(store, claim, "evidence_mismatch").await;
        }
        if snapshot.observed_from < claim.claimed_at
            || !snapshot.valid_at(clock.now())
            || prepared
                .check_current()
                .await
                .inspect_err(|error| {
                    record_error(
                        DiagnosticLevel::Warn,
                        error.diagnostic_kind(),
                        "fulfillment_authorization",
                        "validate_custody_freshness",
                        DiagnosticContext {
                            obligation_id: diagnostic_id(&claim.obligation_id),
                            world: Some(claim.world),
                            ..Default::default()
                        },
                    );
                })
                .is_err()
        {
            record_error(
                DiagnosticLevel::Warn,
                SafeErrorKind::EVIDENCE_EXPIRED,
                "fulfillment_authorization",
                "check_custody_window",
                DiagnosticContext {
                    obligation_id: diagnostic_id(&claim.obligation_id),
                    world: Some(claim.world),
                    defer_reason: Some("evidence_unavailable"),
                    ..Default::default()
                },
            );
            return defer(
                store,
                claim,
                retry_delay,
                "evidence_unavailable",
                "装备 custody 证明不在当前 claim 观察窗口",
            )
            .await;
        }
        // 同一不可重入会话内已读取现金；不得再通过独立 Balance 端口抢锁或换账号。
        if policy.observes_cash()
            && snapshot.main_cash.as_ref().is_none_or(|cash| cash.credits < intent.credits)
        {
            return defer(
                store,
                claim,
                retry_delay,
                "balance_insufficient",
                "同账号 Main Balance 不足以执行当前履约",
            )
            .await;
        }
        return Ok(AuthorizationResult::Authorized((claim, intent, Some(prepared))));
    }

    if intent.credits > 0 && policy.observes_cash() {
        let observed_at = clock.now();
        let balance = match evidence.read_balance(claim.world, observed_at).await {
            Ok(balance) => balance,
            Err(error) => {
                record_error(
                    DiagnosticLevel::Warn,
                    error.diagnostic_kind(),
                    "fulfillment_authorization",
                    "read_balance",
                    DiagnosticContext {
                        obligation_id: diagnostic_id(&claim.obligation_id),
                        world: Some(claim.world),
                        ..Default::default()
                    },
                );

                return defer(
                    store,
                    claim,
                    retry_delay,
                    "balance_unavailable",
                    "MoogleMail Balance 证明暂不可用",
                )
                .await;
            }
        };
        let checked_at = clock.now();
        if validate_balance_evidence(&claim, &balance).is_err() || balance.observed_at > checked_at
        {
            return defer(
                store,
                claim,
                retry_delay,
                "balance_stale",
                "MoogleMail Balance 证明无效或过期",
            )
            .await;
        }
        if balance.credits < intent.credits {
            return defer(
                store,
                claim,
                retry_delay,
                "balance_insufficient",
                "MoogleMail Balance 不足以执行当前履约",
            )
            .await;
        }
    }
    Ok(AuthorizationResult::Authorized((claim, intent, None)))
}

/// enqueued 只复核不可变 UUID 身份；收件地址始终使用原 delivery，不追随改名或失格。
pub(super) async fn authorize_enqueued_recipient(
    store: &dyn SettlementFulfillmentStore,
    recipients: &dyn SettlementFulfillmentRecipientReader,
    claim: SettlementFulfillmentClaim,
    retry_delay: Duration,
) -> Result<AuthorizationResult<SettlementFulfillmentClaim>, ApplicationError> {
    let mut claim = match resolve_recipient(
        store,
        recipients,
        claim,
        retry_delay,
        false,
        "evidence_unavailable",
    )
    .await?
    {
        AuthorizationResult::Authorized(claim) => claim,
        result => return Ok(result),
    };
    let Some(snapshot) = claim.delivery_snapshot.as_ref() else {
        return manual(store, claim, "invalid_fulfillment_fact").await;
    };
    claim.recipient = snapshot.recipient.clone();
    Ok(AuthorizationResult::Authorized(claim))
}

#[allow(clippy::too_many_arguments)]
async fn resolve_recipient(
    store: &dyn SettlementFulfillmentStore,
    recipients: &dyn SettlementFulfillmentRecipientReader,
    mut claim: SettlementFulfillmentClaim,
    retry_delay: Duration,
    require_eligible: bool,
    unavailable_code: &'static str,
) -> Result<AuthorizationResult<SettlementFulfillmentClaim>, ApplicationError> {
    let Some(user_id) = claim.recipient_user_id else {
        return Ok(AuthorizationResult::Authorized(claim));
    };
    let snapshot = match recipients.read_recipient(user_id).await {
        Ok(snapshot) => snapshot,
        // 旧版把 recipient reader 的身份不一致视为当前依赖不可用；阶段专属的
        // unavailable code 由 pending/prepared 与 enqueued 调用方传入，不能提前 terminalize。
        Err(ApplicationError::FulfillmentDependencyMismatch) => {
            record_error(
                DiagnosticLevel::Warn,
                SafeErrorKind::DEPENDENCY_MISMATCH,
                "fulfillment_recipient",
                "validate_dependency",
                DiagnosticContext {
                    obligation_id: diagnostic_id(&claim.obligation_id),
                    world: Some(claim.world),
                    defer_reason: Some(unavailable_code),
                    ..Default::default()
                },
            );
            return defer(store, claim, retry_delay, unavailable_code, "当前履约收件人暂不可用")
                .await;
        }
        Err(error) => {
            record_error(
                DiagnosticLevel::Warn,
                error.diagnostic_kind(),
                "fulfillment_recipient",
                "read_identity",
                DiagnosticContext {
                    obligation_id: diagnostic_id(&claim.obligation_id),
                    world: Some(claim.world),
                    ..Default::default()
                },
            );

            return defer(store, claim, retry_delay, unavailable_code, "当前履约收件人暂不可用")
                .await;
        }
    };
    if snapshot.user_id != user_id {
        record_error(
            DiagnosticLevel::Warn,
            SafeErrorKind::DEPENDENCY_MISMATCH,
            "fulfillment_recipient",
            "validate_user",
            DiagnosticContext {
                obligation_id: diagnostic_id(&claim.obligation_id),
                world: Some(claim.world),
                defer_reason: Some(unavailable_code),
                ..Default::default()
            },
        );
        return defer(store, claim, retry_delay, unavailable_code, "当前履约收件人暂不可用").await;
    }
    if snapshot.user_id.is_nil()
        || snapshot.state_version == 0
        || (require_eligible
            && (snapshot.game_identity.is_empty()
                || snapshot.game_identity != snapshot.game_identity.trim()))
    {
        return manual(store, claim, "invalid_fulfillment_fact").await;
    }
    if require_eligible && !snapshot.eligible {
        record_error(
            DiagnosticLevel::Info,
            SafeErrorKind::DOMAIN_REJECTED,
            "fulfillment_recipient",
            "validate_eligibility",
            DiagnosticContext {
                obligation_id: diagnostic_id(&claim.obligation_id),
                world: Some(claim.world),
                defer_reason: Some(unavailable_code),
                ..Default::default()
            },
        );
        return defer(
            store,
            claim,
            retry_delay,
            unavailable_code,
            "当前履约收件人不再具备执行资格",
        )
        .await;
    }
    if require_eligible {
        claim.recipient = snapshot.game_identity;
    }
    Ok(AuthorizationResult::Authorized(claim))
}

async fn defer<T>(
    store: &dyn SettlementFulfillmentStore,
    claim: SettlementFulfillmentClaim,
    retry_delay: Duration,
    failure_code: &'static str,
    safe_detail: &'static str,
) -> Result<AuthorizationResult<T>, ApplicationError> {
    if retry_delay <= Duration::zero() {
        return Err(repository_error("履约 dependency 重试间隔无效"));
    }
    let obligation_id = claim.obligation_id;
    store.fail(claim, retry_delay, failure_code.to_owned(), safe_detail.to_owned()).await?;
    Ok(AuthorizationResult::Deferred { obligation_id, failure_code })
}

async fn manual<T>(
    store: &dyn SettlementFulfillmentStore,
    claim: SettlementFulfillmentClaim,
    failure_code: &'static str,
) -> Result<AuthorizationResult<T>, ApplicationError> {
    let obligation_id = claim.obligation_id;
    store.manual_reconcile(claim, failure_code.to_owned()).await?;
    Ok(AuthorizationResult::ManualReconcile { obligation_id, failure_code })
}
