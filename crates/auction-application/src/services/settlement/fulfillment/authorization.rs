//! 履约阶段推进前的收件人、custody 与余额授权。

use auction_domain::SettlementObligationKind;
use chrono::Duration;

use crate::{
    ApplicationError, Clock, SettlementFulfillmentClaim, SettlementFulfillmentCustodyReader,
    SettlementFulfillmentEvidenceReader, SettlementFulfillmentOperation,
    SettlementFulfillmentRecipientReader, SettlementFulfillmentStore,
};

use super::evidence::validate_balance_evidence;
use super::intent::build_delivery_intent;
use super::model::SettlementDeliveryIntent;
use super::validation::repository_error;

/// dependency 授权要么返回可继续使用的事实，要么已经持久化等待/人工态。
pub(super) enum AuthorizationResult<T> {
    Authorized(T),
    Deferred { obligation_id: auction_domain::SettlementId, failure_code: &'static str },
    ManualReconcile { obligation_id: auction_domain::SettlementId, failure_code: &'static str },
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
    observe_cash: bool,
) -> Result<
    AuthorizationResult<(SettlementFulfillmentClaim, SettlementDeliveryIntent)>,
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
        let snapshot = match custody
            .read_custody(
                claim.equipment_id,
                claim.equipment_world,
                claim.external_equipment_id.clone(),
            )
            .await
        {
            Ok(snapshot) => snapshot,
            Err(ApplicationError::FulfillmentDependencyMismatch) => {
                return manual(store, claim, "evidence_mismatch").await;
            }
            Err(_) => {
                return defer(
                    store,
                    claim,
                    retry_delay,
                    "evidence_unavailable",
                    "装备 custody 证明暂不可用",
                )
                .await;
            }
        };
        let checked_at = clock.now();
        if snapshot.equipment_id != claim.equipment_id
            || snapshot.world != claim.equipment_world
            || snapshot.external_equipment_id != claim.external_equipment_id
            || snapshot.external_equipment_id.is_empty()
            || snapshot.external_equipment_id != snapshot.external_equipment_id.trim()
            || !snapshot.custodied
            || snapshot.digest == [0; 32]
            || snapshot.observed_at == chrono::DateTime::<chrono::Utc>::default()
            || snapshot.observed_at.timestamp_nanos_opt().is_none()
        {
            return manual(store, claim, "evidence_mismatch").await;
        }
        if snapshot.observed_at < claim.claimed_at || snapshot.observed_at > checked_at {
            return defer(
                store,
                claim,
                retry_delay,
                "evidence_unavailable",
                "装备 custody 证明不在当前 claim 观察窗口",
            )
            .await;
        }
    }

    if intent.credits > 0 && observe_cash {
        let observed_at = clock.now();
        let balance = match evidence.read_balance(claim.world, observed_at).await {
            Ok(balance) => balance,
            Err(_) => {
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
    Ok(AuthorizationResult::Authorized((claim, intent)))
}

/// enqueued 已经发生外部写入，只复核当前身份，不再因之后失格否定既有发送事实。
pub(super) async fn authorize_enqueued_recipient(
    store: &dyn SettlementFulfillmentStore,
    recipients: &dyn SettlementFulfillmentRecipientReader,
    claim: SettlementFulfillmentClaim,
    retry_delay: Duration,
) -> Result<AuthorizationResult<SettlementFulfillmentClaim>, ApplicationError> {
    resolve_recipient(store, recipients, claim, retry_delay, false, "evidence_unavailable").await
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
            return defer(store, claim, retry_delay, unavailable_code, "当前履约收件人暂不可用")
                .await;
        }
        Err(_) => {
            return defer(store, claim, retry_delay, unavailable_code, "当前履约收件人暂不可用")
                .await;
        }
    };
    if snapshot.user_id != user_id {
        return defer(store, claim, retry_delay, unavailable_code, "当前履约收件人暂不可用").await;
    }
    if snapshot.user_id.is_nil()
        || snapshot.game_identity.is_empty()
        || snapshot.game_identity != snapshot.game_identity.trim()
        || snapshot.state_version == 0
    {
        return manual(store, claim, "invalid_fulfillment_fact").await;
    }
    if require_eligible && !snapshot.eligible {
        return defer(
            store,
            claim,
            retry_delay,
            unavailable_code,
            "当前履约收件人不再具备执行资格",
        )
        .await;
    }
    claim.recipient = snapshot.game_identity;
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
