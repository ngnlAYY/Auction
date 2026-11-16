//! 未绑定退件的只读重新核验：领取待核验案件交给 Worker 读取 Bot 收件箱中的原退件，再以读到的
//! 发件人、主题、正文事实与资产重新判定。只有重新满足系统来源、资产与唯一原派发绑定时才改绑为
//! 待决定；否则保持隔离。人工填写的义务或投递不构成证明，证据原文不改写（不变量 6、8）。

use auction_application::core::ApplicationError;
use auction_application::settlement::{
    SettlementReturnFacts, SettlementReturnMatch, SettlementReturnProgress,
    SettlementReturnRecheckClaim, SettlementReturnRecheckOutcome,
    settlement_return_dispatch_request, system_returned_settlement_obligation_id,
};
use auction_domain::SettlementReturnCaseStatus;
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::cases::{CaseEvent, CaseTransition, load_case, transition};
use super::errors::ReturnTransactionError;
use crate::database::repository_error;
use crate::database::settlement::fulfillment::{hold_for_manual_reconcile, lock_obligation};

/// 领取一项待核验案件；租约到期前其他 Worker 不会重复领取。
pub(super) async fn claim_recheck_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
) -> Result<Option<SettlementReturnRecheckClaim>, ApplicationError> {
    claim_once(pool, now, lease_until).await.map_err(ReturnTransactionError::into_application)
}

async fn claim_once(
    pool: &crate::database::TrackedPool<Postgres>,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
) -> Result<Option<SettlementReturnRecheckClaim>, ReturnTransactionError> {
    if lease_until <= now {
        return Err(repository_error("结算退件核验租约无效").into());
    }
    let mut tx = pool.begin().await?;
    let token = Uuid::new_v4();
    let row = sqlx::query(
        r#"WITH due AS (
               SELECT c.return_mail_id
               FROM public.settlement_return_cases AS c
               WHERE c.status = 'recheck_pending'
                 AND (c.recheck_claim_token IS NULL OR c.recheck_claim_expires_at <= $1)
               ORDER BY c.updated_at, c.return_mail_id
               LIMIT 1
               FOR UPDATE SKIP LOCKED
           )
           UPDATE public.settlement_return_cases AS c
           SET recheck_claim_token = $2, recheck_claim_expires_at = $3
           FROM due, public.inbound_mail AS inbound
           WHERE c.return_mail_id = due.return_mail_id AND inbound.id = c.return_mail_id
           RETURNING c.return_mail_id, c.world_id, c.case_version, inbound.external_mail_id"#,
    )
    .bind(now)
    .bind(token)
    .bind(lease_until)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    row.map(|row| {
        Ok(SettlementReturnRecheckClaim {
            return_mail_id: row.try_get("return_mail_id")?,
            world_id: row.try_get("world_id")?,
            external_mail_id: row.try_get("external_mail_id")?,
            case_version: row.try_get("case_version")?,
            claim_token: token,
            lease_until,
        })
    })
    .transpose()
}

/// 写回核验结果；领取已失效或案件已变化时失败关闭。
pub(super) async fn record_recheck_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &SettlementReturnRecheckClaim,
    outcome: &SettlementReturnRecheckOutcome,
) -> Result<SettlementReturnProgress, ApplicationError> {
    crate::database::retry_conflicts(
        pool,
        auction_observability::DatabaseTransactionOwner::Fulfillment,
        crate::database::SERIALIZABLE_ATTEMPTS,
        ReturnTransactionError::retryable,
        || record_once(pool, claim, outcome),
    )
    .await
    .map_err(ReturnTransactionError::into_application)
}

async fn record_once(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &SettlementReturnRecheckClaim,
    outcome: &SettlementReturnRecheckOutcome,
) -> Result<SettlementReturnProgress, ReturnTransactionError> {
    let observed_at = match outcome {
        SettlementReturnRecheckOutcome::Observed(observation) => observation.observed_at,
        SettlementReturnRecheckOutcome::NotFound { observed_at } => *observed_at,
    };
    let mut tx = pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await?;
    let snapshot = load_case(&mut tx, claim.return_mail_id, false)
        .await?
        .ok_or_else(|| repository_error("结算退件案件不存在"))?;
    let fresh = match outcome {
        SettlementReturnRecheckOutcome::Observed(observation) => {
            let obligation_id =
                system_returned_settlement_obligation_id(&observation.sender, &observation.subject);
            let mut metadata = json!({
                "obligation_id": obligation_id.map(|value| value.to_string()),
                "credits": observation.credits,
                "cod": observation.cod,
                "attachment_ids": observation.attachment_ids,
            });
            if let Some(body) = observation.body.as_ref() {
                metadata["returned_from"] = json!(body.returned_from);
                metadata["body_digest"] = json!(
                    body.body_digest.iter().map(|byte| format!("{byte:02x}")).collect::<String>()
                );
                if let Some(reference) = body.dispatch_reference.as_deref() {
                    metadata["dispatch_reference"] = json!(reference);
                }
            }
            // 核验只能证实原证据义务；读到的主题指向别的义务时不改绑。
            let locked = match (obligation_id, snapshot.obligation_id) {
                (Some(observed), Some(recorded)) if observed == recorded => {
                    lock_obligation(
                        &mut tx,
                        recorded,
                        settlement_return_dispatch_request(&metadata),
                    )
                    .await?
                }
                _ => None,
            };
            let received_at: DateTime<Utc> =
                sqlx::query_scalar("SELECT received_at FROM public.inbound_mail WHERE id = $1")
                    .bind(claim.return_mail_id)
                    .fetch_one(&mut *tx)
                    .await?;
            let facts = SettlementReturnFacts {
                sender: Some(observation.sender.clone()),
                subject: Some(observation.subject.clone()),
                return_world: snapshot.world_id.clone(),
                received_at,
                metadata,
                obligation: locked.as_ref().map(|locked| locked.facts.clone()),
            };
            Some((facts.classify(), locked))
        }
        SettlementReturnRecheckOutcome::NotFound { .. } => None,
    };
    let case = load_case(&mut tx, claim.return_mail_id, true)
        .await?
        .ok_or_else(|| repository_error("结算退件案件不存在"))?;
    let token: Option<Uuid> = sqlx::query_scalar(
        "SELECT recheck_claim_token FROM public.settlement_return_cases WHERE return_mail_id = $1",
    )
    .bind(claim.return_mail_id)
    .fetch_one(&mut *tx)
    .await?;
    if case.status != SettlementReturnCaseStatus::RecheckPending
        || case.case_version != claim.case_version
        || token != Some(claim.claim_token)
        || observed_at > claim.lease_until
        || case.match_state == SettlementReturnMatch::Untrusted
    {
        return Err(repository_error("结算退件核验领取已失效").into());
    }
    let (next, kind, details) = match fresh {
        Some((decision, Some(locked))) if decision.matched.bound() => {
            let delivery = locked
                .facts
                .delivery
                .as_ref()
                .ok_or_else(|| repository_error("结算退件改绑缺少原派发"))?;
            if decision.matched == SettlementReturnMatch::ManualReconcile {
                hold_for_manual_reconcile(
                    &mut tx,
                    &locked,
                    claim.return_mail_id,
                    decision,
                    observed_at,
                )
                .await?;
            }
            let mut next =
                CaseTransition::from_case(&case, SettlementReturnCaseStatus::AwaitingDecision);
            next.delivery_id = Some(delivery.id);
            next.dispatch_no = i32::try_from(delivery.dispatch_no).ok();
            next.reason_code = decision.reason.as_str().to_owned();
            next.recheck_completed = true;
            (
                next,
                "recheck_rebound",
                json!({
                    "match_state": decision.matched.as_str(),
                    "reason_code": decision.reason.as_str(),
                    "delivery_id": delivery.id,
                    "dispatch_no": delivery.dispatch_no,
                }),
            )
        }
        Some((decision, _)) => {
            let mut next =
                CaseTransition::from_case(&case, SettlementReturnCaseStatus::Quarantined);
            next.recheck_completed = true;
            (
                next,
                "recheck_unchanged",
                json!({
                    "match_state": decision.matched.as_str(),
                    "reason_code": decision.reason.as_str(),
                }),
            )
        }
        None => {
            let mut next =
                CaseTransition::from_case(&case, SettlementReturnCaseStatus::Quarantined);
            next.recheck_completed = true;
            (next, "recheck_unchanged", json!({"reason_code": "mail_not_found"}))
        }
    };
    let status = next.status;
    let version =
        transition(&mut tx, &case, &next, CaseEvent { kind, command: None, details }, observed_at)
            .await?;
    tx.commit().await?;
    Ok(SettlementReturnProgress {
        return_mail_id: claim.return_mail_id,
        status,
        case_version: version,
        event_kind: kind,
    })
}
