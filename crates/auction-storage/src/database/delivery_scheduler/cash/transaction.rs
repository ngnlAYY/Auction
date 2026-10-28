//! 现金fence的明确提交/回滚及有界串行化重试；保留A/B原始冲突分类。

use std::time::Instant;

use auction_application::{
    ApplicationError, DeliveryFenceOutcome, DeliveryPreparedExpectation, DeliveryWorkClaim,
    FundingError, SettlementInternalEvidenceSnapshot,
};
use chrono::{DateTime, Utc};
use sqlx::Postgres;

use crate::{MailEvidenceTransactionError, database::TrackedPool};

pub(in crate::database::delivery_scheduler) enum FenceError {
    Sqlx(sqlx::Error),
    Funding(FundingError),
    Evidence(MailEvidenceTransactionError),
    Rejected,
}

impl From<sqlx::Error> for FenceError {
    fn from(error: sqlx::Error) -> Self {
        Self::Sqlx(error)
    }
}
impl From<FundingError> for FenceError {
    fn from(error: FundingError) -> Self {
        Self::Funding(error)
    }
}
impl From<MailEvidenceTransactionError> for FenceError {
    fn from(error: MailEvidenceTransactionError) -> Self {
        Self::Evidence(error)
    }
}
impl FenceError {
    fn retryable(&self) -> bool {
        match self {
            Self::Sqlx(error) => crate::database::is_serialization_conflict(error),
            Self::Funding(FundingError::SerializationConflict) => true,
            Self::Evidence(error) => error.is_serialization_conflict(),
            _ => false,
        }
    }
}

fn uncertain() -> ApplicationError {
    ApplicationError::Repository(auction_application::RepositoryError::Failed(
        "现金sending fence未确定完成，保留原命令并核对持久状态".to_owned(),
    ))
}

/// 同一个 commit guard 覆盖最多三次重试；Deferred 仅表示确定未写入/回滚或已提交冻结。
pub(in crate::database::delivery_scheduler) async fn run(
    pool: &TrackedPool<Postgres>,
    claim: &DeliveryWorkClaim,
    started_at: DateTime<Utc>,
    evidence: Option<&SettlementInternalEvidenceSnapshot>,
    deadline: Option<Instant>,
    intervals: &auction_application::DeliverySendIntervals,
    expectation: Option<&DeliveryPreparedExpectation>,
) -> Result<DeliveryFenceOutcome, ApplicationError> {
    if evidence.is_some() != deadline.is_some() {
        return Err(uncertain());
    }
    run_transaction(pool, claim, started_at, evidence, deadline, intervals, expectation).await
}

/// legacy沿用原返回类型，只有明确Sending才成功，缺授权/确定拒绝不伪装成功。
pub(in crate::database::delivery_scheduler) async fn run_legacy(
    pool: &TrackedPool<Postgres>,
    claim: &DeliveryWorkClaim,
    started_at: DateTime<Utc>,
    intervals: &auction_application::DeliverySendIntervals,
) -> Result<(), auction_application::RepositoryError> {
    match run_transaction(pool, claim, started_at, None, None, intervals, None).await {
        Ok(DeliveryFenceOutcome::Sending) => Ok(()),
        _ => Err(auction_application::RepositoryError::Failed(
            "delivery发送fence未授权或未确定完成".to_owned(),
        )),
    }
}

async fn run_transaction(
    pool: &TrackedPool<Postgres>,
    claim: &DeliveryWorkClaim,
    started_at: DateTime<Utc>,
    evidence: Option<&SettlementInternalEvidenceSnapshot>,
    deadline: Option<Instant>,
    intervals: &auction_application::DeliverySendIntervals,
    expectation: Option<&DeliveryPreparedExpectation>,
) -> Result<DeliveryFenceOutcome, ApplicationError> {
    let matches_claim = match expectation {
        Some(DeliveryPreparedExpectation::Send(expectation)) => {
            super::super::attempts::matches_claim(claim, Some(expectation))
        }
        Some(DeliveryPreparedExpectation::Take(expectation)) => {
            super::super::take_attempts::matches_claim(claim, expectation)
        }
        None => {
            super::super::attempts::matches_claim(claim, None)
                && claim.payload.get("operation").and_then(serde_json::Value::as_str)
                    != Some("take")
        }
    };
    if !matches_claim {
        return Ok(DeliveryFenceOutcome::Deferred);
    }
    for attempt in 0..3 {
        if deadline.is_some_and(|limit| Instant::now() >= limit) {
            // 尚未开始事务的确定拒绝；重试不能重建证据寿命。
            return Ok(DeliveryFenceOutcome::Deferred);
        }
        let mut tx =
            pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await.map_err(|_| uncertain())?;
        let result = async {
            if let (Some(deadline), Some(evidence)) = (deadline, evidence) {
                let remaining =
                    deadline.checked_duration_since(Instant::now()).ok_or(FenceError::Rejected)?;
                let evidence_remaining = (evidence.expires_at - Utc::now())
                    .to_std()
                    .map_err(|_| FenceError::Rejected)?;
                let millis = remaining.min(evidence_remaining).as_millis();
                if millis == 0 {
                    return Err(FenceError::Rejected);
                }
                // PostgreSQL 18 的事务超时覆盖后续锁、审计和 COMMIT。
                // 原单调截止与 UTC 证据窗口只取交集，不替代原数据库实时时效校验。
                // 向下取整保守收紧，零会禁用超时，必须拒绝；每次只用同一截止的剩余预算。
                sqlx::query("SELECT set_config('transaction_timeout', $1, true)")
                    .bind(format!("{millis}ms"))
                    .execute(&mut *tx)
                    .await?;
            }
            // 分类节奏锁在全部业务锁之前；未到期确定回滚，不在持有凭据期间睡眠。
            let category = super::super::cadence::lock_due(&mut tx, claim, intervals).await?;
            let outcome = match evidence {
                Some(evidence) => {
                    super::authorization::authorize(
                        &mut tx,
                        claim,
                        started_at,
                        evidence,
                        expectation,
                    )
                    .await
                }
                None => {
                    super::authorization::authorize_legacy(&mut tx, claim, started_at, expectation)
                        .await
                }
            }?;
            if matches!(outcome, DeliveryFenceOutcome::Sending)
                && let Some(category) = category
            {
                super::super::cadence::record_started(&mut tx, &category).await?;
            }
            Ok::<_, FenceError>(outcome)
        }
        .await;
        match result {
            Ok(_) if deadline.is_some_and(|limit| Instant::now() >= limit) => {
                tx.rollback().await.map_err(|_| uncertain())?;
                return Ok(DeliveryFenceOutcome::Deferred);
            }
            Ok(outcome) => match tx.commit().await {
                Ok(()) => {
                    // 即使服务器已提交，过期后才收到回执也不能授权外部发送。
                    if deadline.is_some_and(|limit| Instant::now() >= limit) {
                        return Err(uncertain());
                    }
                    return Ok(outcome);
                }
                Err(ref error) if crate::database::is_serialization_conflict(error) => {
                    // PostgreSQL明确中止的冲突可重试；普通连接/提交错误必须撤销owner。
                    if attempt == 2 {
                        return Ok(DeliveryFenceOutcome::Deferred);
                    }
                }
                Err(_) => return Err(uncertain()),
            },
            Err(error) => {
                let retry = error.retryable() && attempt < 2;
                tx.rollback().await.map_err(|_| uncertain())?;
                if !retry {
                    return Ok(DeliveryFenceOutcome::Deferred);
                }
            }
        }
    }
    Ok(DeliveryFenceOutcome::Deferred)
}
