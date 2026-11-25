//! 现金fence的明确提交/回滚及有界串行化重试；保留A/B原始冲突分类。回滚已确认时按失败原因
//! 区分业务门延期与带安全分类的内部故障；开启、回滚或提交结果未知仍以错误返回，由调用方撤销owner。

use std::time::Instant;

use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::{DiagnosticError, SafeErrorKind};
use auction_application::delivery::{
    DeliveryFenceOutcome, DeliveryPreparedExpectation, DeliveryWorkClaim,
};
use auction_application::settlement::{FundingError, SettlementInternalEvidenceSnapshot};
use auction_time::Timestamp;
use sqlx::Postgres;

use crate::{MailEvidenceTransactionError, database::TrackedPool};

pub(in crate::database::delivery::scheduler) enum FenceError {
    Sqlx(sqlx::Error),
    Funding(FundingError),
    Evidence(MailEvidenceTransactionError),
    /// 业务门确定未满足：竞价警告失效、发送节奏未到期、claim 已失效或与持久行不符、资金门不放行等。
    Rejected,
    /// 不属于业务门的确定故障：提交预算耗尽、编码失败或持久不变量损坏；回滚后按所带分类保留错误信号。
    Fault(SafeErrorKind),
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
impl From<super::super::reminder_gate::GateError> for FenceError {
    fn from(error: super::super::reminder_gate::GateError) -> Self {
        use super::super::reminder_gate::GateError;
        use crate::database::consignment::commands::errors::WriteError;
        match error {
            GateError::Sqlx(error) => Self::Sqlx(error),
            // 提醒事实读取遇到序列化冲突或持久失配：回滚后带分类返回，不写 sending。
            GateError::Storage(WriteError::RetryConflict) => {
                Self::Fault(SafeErrorKind::REPOSITORY_SERIALIZATION_CONFLICT)
            }
            GateError::Storage(_) => Self::Fault(SafeErrorKind::REPOSITORY_FAILED),
        }
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
            Self::Sqlx(error) => crate::database::observed_serialization_conflict(error),
            Self::Funding(FundingError::SerializationConflict) => true,
            Self::Evidence(error) => error.is_serialization_conflict(),
            _ => false,
        }
    }

    /// 回滚已确认后的分类：业务门拒绝（含资金 owner 的确定拒绝，以及证据 owner 判定凭据或运行时
    /// owner 在观察之后正常推进）返回 `None`，按正常延期收口；SQL 错误、冲突重试耗尽、证据结构、
    /// generation、时效或绑定异常与其余确定故障返回安全分类，不能被延期掩盖。
    fn fault_kind(&self) -> Option<SafeErrorKind> {
        match self {
            Self::Evidence(MailEvidenceTransactionError::Rejected(rejection)) => {
                let kind = rejection.fault_kind();
                if let Some(kind) = kind {
                    // 证据 owner 的原因到此为止：同一分类下的 generation 与结构异常只靠原因标签
                    // 区分；只记低基数标签，不记证据内容。
                    tracing::warn!(
                        reason = rejection.label(),
                        safe_kind = kind.label(),
                        "现金 sending fence 的邮件证据被拒绝，事务已回滚"
                    );
                }
                kind
            }
            Self::Rejected
            | Self::Funding(
                FundingError::Invalid
                | FundingError::Conflict
                | FundingError::Unavailable
                | FundingError::Unproven
                | FundingError::Unsupported,
            ) => None,
            Self::Funding(
                error @ (FundingError::SerializationConflict | FundingError::Storage),
            ) => Some(error.diagnostic_kind()),
            Self::Sqlx(error) | Self::Evidence(MailEvidenceTransactionError::Sqlx(error)) => {
                let kind = if crate::database::backend::is_serialization_conflict(error) {
                    SafeErrorKind::REPOSITORY_SERIALIZATION_CONFLICT
                } else {
                    SafeErrorKind::REPOSITORY_FAILED
                };
                // 原 SQL 错误到此为止：先记录 SQLSTATE 等安全投影，排障不依赖 Worker 的固定分类。
                crate::database::log_discarded_sql_error(
                    error,
                    kind.label(),
                    "现金 sending fence 事务失败并已回滚",
                );
                Some(kind)
            }
            Self::Fault(kind) => Some(*kind),
        }
    }
}

/// 观测边界内的非成功结局：确定回滚的内部故障按事务错误计量，结果未知另行保守处理。
enum FenceStop {
    /// 事务已确定回滚或从未开始，没有写入 sending。
    Faulted(SafeErrorKind),
    /// 开启、回滚或提交结果未知，或提交回执晚于截止。
    Uncertain,
}

fn uncertain() -> ApplicationError {
    ApplicationError::Repository(auction_application::core::RepositoryError::Failed(
        "现金sending fence未确定完成，保留原命令并核对持久状态".to_owned(),
    ))
}

/// 同一个 commit guard 覆盖最多三次重试；Deferred 仅表示业务门确定未写入/回滚或已提交冻结，
/// Faulted 表示内部故障后确定回滚。
pub(in crate::database::delivery::scheduler) async fn run(
    pool: &TrackedPool<Postgres>,
    claim: &DeliveryWorkClaim,
    started_at: Timestamp,
    evidence: Option<&SettlementInternalEvidenceSnapshot>,
    deadline: Option<Instant>,
    intervals: &auction_application::delivery::DeliverySendIntervals,
    expectation: Option<&DeliveryPreparedExpectation>,
) -> Result<DeliveryFenceOutcome, ApplicationError> {
    if evidence.is_some() != deadline.is_some() {
        return Err(uncertain());
    }
    // 确定回滚的内部故障在观测边界内按事务错误计量，出边界后才转成带分类的确定结论。
    match run_transaction(pool, claim, started_at, evidence, deadline, intervals, expectation).await
    {
        Ok(outcome) => Ok(outcome),
        Err(FenceStop::Faulted(kind)) => Ok(DeliveryFenceOutcome::Faulted(kind)),
        Err(FenceStop::Uncertain) => Err(uncertain()),
    }
}

async fn run_transaction(
    pool: &TrackedPool<Postgres>,
    claim: &DeliveryWorkClaim,
    started_at: Timestamp,
    evidence: Option<&SettlementInternalEvidenceSnapshot>,
    deadline: Option<Instant>,
    intervals: &auction_application::delivery::DeliverySendIntervals,
    expectation: Option<&DeliveryPreparedExpectation>,
) -> Result<DeliveryFenceOutcome, FenceStop> {
    pool.observe_transaction(
        auction_observability::DatabaseTransactionOwner::DeliveryFence,
        async {
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
                // 准备产出的冻结身份与 claim 不符是适配器不变量损坏，不是业务门；事务尚未开始。
                return Err(FenceStop::Faulted(SafeErrorKind::DEPENDENCY_MISMATCH));
            }
            for attempt in 0..3 {
                if deadline.is_some_and(|limit| Instant::now() >= limit) {
                    // 尚未开始事务即耗尽提交预算；重试不能重建证据寿命。
                    return Err(FenceStop::Faulted(SafeErrorKind::TIMEOUT));
                }
                let mut tx = pool
                    .begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE")
                    .await
                    .map_err(|_| FenceStop::Uncertain)?;
                let result = async {
                    if let (Some(deadline), Some(evidence)) = (deadline, evidence) {
                        // 提交预算取原单调截止与证据寿命的交集；任一已耗尽都是确定故障，不是业务门。
                        let remaining = deadline
                            .checked_duration_since(Instant::now())
                            .ok_or(FenceError::Fault(SafeErrorKind::TIMEOUT))?;
                        let evidence_remaining = (evidence.expires_at - Timestamp::now())
                            .to_std()
                            .map_err(|_| FenceError::Fault(SafeErrorKind::TIMEOUT))?;
                        let millis = remaining.min(evidence_remaining).as_millis();
                        if millis == 0 {
                            return Err(FenceError::Fault(SafeErrorKind::TIMEOUT));
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
                    let category =
                        super::super::cadence::lock_due(&mut tx, claim, intervals).await?;
                    // 付款提醒的派前复验：已付、已取消或轮已变时不写 sending（零发送）。
                    if !super::super::reminder_gate::allows_sending(&mut tx, claim).await? {
                        return Err(FenceError::Rejected);
                    }
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
                            super::authorization::authorize_legacy(
                                &mut tx,
                                claim,
                                started_at,
                                expectation,
                            )
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
                        tx.rollback().await.map_err(|_| FenceStop::Uncertain)?;
                        return Err(FenceStop::Faulted(SafeErrorKind::TIMEOUT));
                    }
                    Ok(outcome) => match tx.commit().await {
                        Ok(()) => {
                            // 即使服务器已提交，过期后才收到回执也不能授权外部发送。
                            if deadline.is_some_and(|limit| Instant::now() >= limit) {
                                return Err(FenceStop::Uncertain);
                            }
                            return Ok(outcome);
                        }
                        Err(ref error)
                            if crate::database::observed_serialization_conflict(error) =>
                        {
                            // PostgreSQL明确中止的冲突可重试；普通连接/提交错误必须撤销owner。
                            if attempt == 2 {
                                auction_observability::database_transaction_exhausted();
                                return Err(FenceStop::Faulted(
                                    SafeErrorKind::REPOSITORY_SERIALIZATION_CONFLICT,
                                ));
                            }
                        }
                        Err(_) => return Err(FenceStop::Uncertain),
                    },
                    Err(error) => {
                        let retry = error.retryable() && attempt < 2;
                        tx.rollback().await.map_err(|_| FenceStop::Uncertain)?;
                        if !retry {
                            crate::database::observe_retry_exhaustion(
                                attempt + 1,
                                3,
                                error.retryable(),
                            );
                            // 回滚已确认：业务门拒绝是正常延期，其余故障带分类返回。
                            return match error.fault_kind() {
                                None => Ok(DeliveryFenceOutcome::Deferred),
                                Some(kind) => Err(FenceStop::Faulted(kind)),
                            };
                        }
                    }
                }
            }
            // 每次尝试都已返回或进入下一次；循环自然结束只可能是冲突重试耗尽。
            Err(FenceStop::Faulted(SafeErrorKind::REPOSITORY_SERIALIZATION_CONFLICT))
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use auction_application::core::diagnostics::SafeErrorKind;
    use auction_application::settlement::FundingError;

    use super::FenceError;
    use crate::{MailEvidenceRejection, MailEvidenceTransactionError};

    /// 回滚后只有业务门拒绝按正常延期收口；SQL 错误、冲突耗尽与确定故障都必须带分类，
    /// 否则 Worker 会把真实故障记成成功（A4）。证据 owner 的拒绝只有“观察之后正常失效”算业务门，
    /// generation、结构、绑定与时效异常都是故障。
    #[test]
    fn only_business_gate_rejections_roll_back_as_deferrals() {
        let deferred = [
            FenceError::Rejected,
            FenceError::Evidence(MailEvidenceTransactionError::Rejected(
                MailEvidenceRejection::Stale,
            )),
            FenceError::Funding(FundingError::Invalid),
            FenceError::Funding(FundingError::Conflict),
            FenceError::Funding(FundingError::Unavailable),
            FenceError::Funding(FundingError::Unproven),
            FenceError::Funding(FundingError::Unsupported),
        ];
        for error in &deferred {
            assert_eq!(error.fault_kind(), None);
        }
        let faults = [
            (
                FenceError::Sqlx(sqlx::Error::Protocol("注入的协议错误".to_owned())),
                SafeErrorKind::REPOSITORY_FAILED,
            ),
            (
                FenceError::Evidence(MailEvidenceTransactionError::Sqlx(sqlx::Error::RowNotFound)),
                SafeErrorKind::REPOSITORY_FAILED,
            ),
            (FenceError::Funding(FundingError::Storage), SafeErrorKind::FUNDING_STORAGE_ERROR),
            (
                FenceError::Funding(FundingError::SerializationConflict),
                SafeErrorKind::FUNDING_SERIALIZATION_CONFLICT,
            ),
            (FenceError::Fault(SafeErrorKind::TIMEOUT), SafeErrorKind::TIMEOUT),
            (
                FenceError::Evidence(MailEvidenceTransactionError::Rejected(
                    MailEvidenceRejection::GenerationInvalid,
                )),
                SafeErrorKind::EVIDENCE_REQUEST_INVALID,
            ),
            (
                FenceError::Evidence(MailEvidenceTransactionError::Rejected(
                    MailEvidenceRejection::StructureInvalid,
                )),
                SafeErrorKind::EVIDENCE_REQUEST_INVALID,
            ),
            (
                FenceError::Evidence(MailEvidenceTransactionError::Rejected(
                    MailEvidenceRejection::BindingMismatch,
                )),
                SafeErrorKind::DEPENDENCY_MISMATCH,
            ),
            (
                FenceError::Evidence(MailEvidenceTransactionError::Rejected(
                    MailEvidenceRejection::TimeInvalid,
                )),
                SafeErrorKind::EVIDENCE_EXPIRED,
            ),
        ];
        for (error, kind) in &faults {
            assert_eq!(error.fault_kind(), Some(*kind));
        }
    }
}
