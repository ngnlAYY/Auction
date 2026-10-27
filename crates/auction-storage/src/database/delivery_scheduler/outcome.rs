//! delivery终态与资金结果同事务；Sent支出，失败/未知均不释放资金。

use super::mapping::*;
use super::*;
use crate::database::settlement_funding as funding;

pub(super) async fn succeed_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &DeliveryWorkClaim,
    result: &DeliverySendResult,
    completed_at: DateTime<Utc>,
) -> Result<(), auction_application::RepositoryError> {
    if claim.payload.get("operation").and_then(serde_json::Value::as_str) == Some("send")
        && !verified_sent_result(claim, result)
    {
        return Err(repository_error_owned("MoogleMail send成功结果缺少完整Sent核验"));
    }
    if claim.origin_kind == "settlement"
        && result.result_metadata.get("operation").and_then(serde_json::Value::as_str)
            != Some("send")
    {
        // 旧return保留原确定动作证明门，不凭人工引用生成Sent。
        if result.result_metadata.get("operation").and_then(serde_json::Value::as_str)
            != Some("return")
            || result
                .result_metadata
                .get("external_mail_id")
                .and_then(serde_json::Value::as_str)
                .is_none()
            || result.result_metadata.get("proof").and_then(serde_json::Value::as_str)
                != Some("action_removed_or_success_page")
        {
            return Err(repository_error_owned("结算delivery成功结果缺少动作证明"));
        }
    }
    persist(
        pool,
        claim,
        completed_at,
        Terminal {
            status: "succeeded",
            reference: Some(&result.external_reference),
            failure: None,
            metadata: &result.result_metadata,
            audit: SUCCEED,
            preparation_rejected: false,
            funding: auction_application::FundingDeliveryOutcome::Sent,
        },
    )
    .await
}

/// 确定未开始外部写入的非法负载直接失败；不伪造发送围栏，也不释放资金责任。
pub(super) async fn reject_preparation_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &DeliveryWorkClaim,
    completed_at: DateTime<Utc>,
) -> Result<(), auction_application::RepositoryError> {
    let metadata = serde_json::json!({
        "stage": "preflight_blocked",
        "write_started": false,
        "outcome_unknown": false,
        "failure_code": "invalid_payload",
    });
    persist(
        pool,
        claim,
        completed_at,
        Terminal {
            status: "failed",
            reference: None,
            failure: Some("invalid_payload"),
            metadata: &metadata,
            audit: FAIL,
            preparation_rejected: true,
            funding: auction_application::FundingDeliveryOutcome::Unknown,
        },
    )
    .await
}

pub(super) async fn fail_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &DeliveryWorkClaim,
    failure_code: &str,
    outcome_unknown: bool,
    external_reference: Option<&str>,
    result_metadata: &serde_json::Value,
    completed_at: DateTime<Utc>,
) -> Result<(), auction_application::RepositoryError> {
    persist(
        pool,
        claim,
        completed_at,
        Terminal {
            status: if outcome_unknown { "manual_reconcile" } else { "failed" },
            reference: external_reference,
            failure: Some(failure_code),
            metadata: result_metadata,
            audit: if outcome_unknown { OUTCOME_UNKNOWN } else { FAIL },
            preparation_rejected: false,
            funding: auction_application::FundingDeliveryOutcome::Unknown,
        },
    )
    .await
}

struct Terminal<'a> {
    status: &'static str,
    reference: Option<&'a str>,
    failure: Option<&'a str>,
    metadata: &'a serde_json::Value,
    audit: &'static str,
    preparation_rejected: bool,
    funding: auction_application::FundingDeliveryOutcome,
}

async fn persist(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &DeliveryWorkClaim,
    completed_at: DateTime<Utc>,
    terminal: Terminal<'_>,
) -> Result<(), auction_application::RepositoryError> {
    for attempt in 0..3 {
        match persist_once(pool, claim, completed_at, &terminal).await {
            Ok(()) => return Ok(()),
            Err(error) if error.retryable() && attempt < 2 => continue,
            Err(_) => {
                return Err(repository_error_owned(
                    "delivery终态未确定写入，保留资金占用并核对原记录",
                ));
            }
        }
    }
    Err(repository_error_owned("delivery终态重试已耗尽"))
}

async fn persist_once(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &DeliveryWorkClaim,
    completed_at: DateTime<Utc>,
    terminal: &Terminal<'_>,
) -> Result<(), OutcomeError> {
    let mut tx = pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await?;
    // 无远端身份也只能使用持久关联，frozen/spent/returned均不能丢失绑定。
    let account = funding::lock_funded_delivery_account(&mut tx, claim.delivery_id).await?;
    sqlx::query("SELECT id FROM public.deliveries WHERE id=$1 FOR UPDATE")
        .bind(claim.delivery_id)
        .fetch_optional(&mut *tx)
        .await?;
    if terminal.audit == SUCCEED
        && claim.payload.get("operation").and_then(serde_json::Value::as_str) == Some("send")
    {
        let expectation = super::attempts::load(&mut tx, claim.delivery_id, claim.fencing_version)
            .await?
            .ok_or(OutcomeError::Rejected)?;
        if !super::attempts::matches_claim(claim, Some(&expectation))
            || !super::attempts::matches_proof(
                terminal.metadata,
                &expectation,
                claim.fencing_version,
            )
        {
            return Err(OutcomeError::Rejected);
        }
        let prefix = format!("mooglemail:{}:send:", expectation.world_id);
        let mid = terminal
            .reference
            .and_then(|reference| reference.strip_prefix(&prefix))
            .ok_or(OutcomeError::Rejected)?;
        if !super::attempts::bind_sent(
            &mut tx,
            claim.delivery_id,
            claim.fencing_version,
            &expectation.world_id,
            mid,
            true,
        )
        .await?
        {
            return Err(OutcomeError::Rejected);
        }
        // 提交应答丢失后的同一事实重放允许成功，但不重新扣资金或追加成功审计。
        let repeated: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM public.deliveries
             WHERE id=$1 AND status='succeeded' AND fencing_version=$2
               AND command_id=$3 AND request_digest=$4 AND origin_kind=$5 AND origin_id=$6
               AND recipient=$7 AND payload=$8 AND external_reference=$9 AND result_metadata=$10)",
        )
        .bind(claim.delivery_id)
        .bind(claim.fencing_version)
        .bind(claim.command_id)
        .bind(claim.request_hash.as_bytes().as_slice())
        .bind(&claim.origin_kind)
        .bind(&claim.origin_id)
        .bind(&claim.recipient)
        .bind(&claim.payload)
        .bind(terminal.reference)
        .bind(terminal.metadata)
        .fetch_one(&mut *tx)
        .await?;
        if repeated {
            tx.rollback().await?;
            return Ok(());
        }
    }
    // 传输结果替换旧证明，但必须保留自动维护已消费的重排预算及其原始 JSON 类型。
    let updated = sqlx::query(
        "UPDATE public.deliveries SET status=$2,lease_token=NULL,lease_expires_at=NULL,
         completed_at=$3,external_reference=$4,failure_code=$5,
         result_metadata=$6::jsonb || CASE WHEN result_metadata ? 'sweep_retries'
             THEN jsonb_build_object('sweep_retries',result_metadata->'sweep_retries')
             ELSE '{}'::jsonb END,
         updated_at=clock_timestamp()
         WHERE id=$1 AND status=$15 AND lease_token=$7 AND fencing_version=$8
         AND lease_expires_at>$3 AND lease_expires_at>clock_timestamp()
         AND command_id=$9 AND request_digest=$10 AND origin_kind=$11 AND origin_id=$12
         AND recipient=$13 AND payload=$14
         AND (NOT $16 OR (sending_at IS NULL AND external_reference IS NULL))",
    )
    .bind(claim.delivery_id)
    .bind(terminal.status)
    .bind(completed_at)
    .bind(terminal.reference)
    .bind(terminal.failure)
    .bind(terminal.metadata)
    .bind(claim.lease_token)
    .bind(claim.fencing_version)
    .bind(claim.command_id)
    .bind(claim.request_hash.as_bytes().as_slice())
    .bind(&claim.origin_kind)
    .bind(&claim.origin_id)
    .bind(&claim.recipient)
    .bind(&claim.payload)
    .bind(if terminal.preparation_rejected { "claimed" } else { "sending" })
    .bind(terminal.preparation_rejected)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(OutcomeError::Rejected);
    }
    if !terminal.preparation_rejected
        && let Some(account) = account
    {
        funding::finish_funded_delivery(&mut tx, &account, claim.delivery_id, terminal.funding)
            .await?;
    }
    let request_id = claim.command_id.to_string();
    let mut audit_metadata = worker_state_metadata(
        terminal.status,
        claim.fencing_version,
        claim.attempt_count,
        terminal.failure,
    );
    if terminal.preparation_rejected {
        audit_metadata["preflight_blocked"] = serde_json::json!(true);
        audit_metadata["write_started"] = serde_json::json!(false);
        audit_metadata["outcome_unknown"] = serde_json::json!(false);
    }
    append_delivery_audit(
        &mut tx,
        DeliveryAudit {
            occurred_at: completed_at,
            actor_type: "worker",
            actor_id: "delivery-worker",
            action: terminal.audit,
            delivery_id: claim.delivery_id,
            request_id: Some(&request_id),
            source: "worker",
            metadata: audit_metadata,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// 各资金终态runner保留SQLSTATE/B分类直到最外层，不匹配错误字符串。
pub(super) enum OutcomeError {
    Sqlx(sqlx::Error),
    Funding(auction_application::FundingError),
    Rejected,
}
impl From<sqlx::Error> for OutcomeError {
    fn from(error: sqlx::Error) -> Self {
        Self::Sqlx(error)
    }
}
impl From<auction_application::FundingError> for OutcomeError {
    fn from(error: auction_application::FundingError) -> Self {
        Self::Funding(error)
    }
}
impl OutcomeError {
    pub(super) fn retryable(&self) -> bool {
        match self {
            Self::Sqlx(error) => error
                .as_database_error()
                .and_then(|error| error.code())
                .is_some_and(|code| matches!(code.as_ref(), "40001" | "40P01")),
            Self::Funding(auction_application::FundingError::SerializationConflict) => true,
            _ => false,
        }
    }
}

fn verified_sent_result(claim: &DeliveryWorkClaim, result: &DeliverySendResult) -> bool {
    let Some(world @ ("main" | "isekai")) =
        claim.payload.get("world").and_then(serde_json::Value::as_str)
    else {
        return false;
    };
    let prefix = format!("mooglemail:{world}:send:");
    let Some(mail_id) = result.external_reference.strip_prefix(&prefix) else {
        return false;
    };
    !mail_id.is_empty()
        && mail_id.len() <= 256
        && mail_id == mail_id.trim()
        && !mail_id.chars().any(char::is_control)
        && result.result_metadata.get("operation").and_then(serde_json::Value::as_str)
            == Some("send")
        && result
            .result_metadata
            .get("external_reference_proven")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        && result.result_metadata.get("sent_verified").and_then(serde_json::Value::as_bool)
            == Some(true)
        && result.result_metadata.get("proof").and_then(serde_json::Value::as_str)
            == Some("sent_exact_payload_v1")
}
