//! 结算退件 Take 结果未知后的原会话观察留证事务：锁验观察凭据与运行代际后，由退件案件 owner
//! 追加 `take_observed` 事件；不改写投递、动作、资金或证据，也不追加投递审计。

use auction_application::delivery::{DeliverySendCredentialBinding, DeliveryTakeExpectation};
use auction_domain::SettlementReturnTakeUnknownReason;
use chrono::{DateTime, Utc};
use sqlx::Postgres;
use uuid::Uuid;

use super::error::RecoveryError;
use super::recovery;
use crate::database::TrackedPool;
use crate::database::delivery::scheduler::attempts;
use crate::database::settlement::returns::{TakeObservation, record_take_observation};

/// 记录一次原会话观察；只重试明确冲突，返回 true 表示本次实际追加了事件。
pub(super) async fn record_return_take_observation(
    pool: &TrackedPool<Postgres>,
    observation: &TakeObservation<'_>,
    observed: &DeliverySendCredentialBinding,
) -> Result<bool, auction_application::core::ApplicationError> {
    crate::database::retry_conflicts(
        pool,
        auction_observability::DatabaseTransactionOwner::MailboxSweep,
        crate::database::SERIALIZABLE_ATTEMPTS,
        RecoveryError::retryable,
        || record_once(pool, observation, observed),
    )
    .await
    .map_err(|error| error.into_application_error("结算退件 Take 观察留证事务未确定提交"))
}

async fn record_once(
    pool: &TrackedPool<Postgres>,
    observation: &TakeObservation<'_>,
    observed: &DeliverySendCredentialBinding,
) -> Result<bool, RecoveryError> {
    let mut tx = pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await?;
    // 锁序：观测 generation → 凭据 → 案件 → 动作 → 投递。只有原 Take 会话（同凭据 ID 与版本、
    // 当前运行代际）的观察才能留证；换账号或换版本的观察在这里拒绝。
    recovery::lock_take_observation(&mut tx, observation.expectation, observed).await?;
    if !attempts::lock_credential_binding(
        &mut tx,
        &observation.expectation.world_id,
        observation.expectation.credential.as_ref(),
    )
    .await?
    {
        return Err(RecoveryError::Rejected);
    }
    let now: DateTime<Utc> =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(&mut *tx).await?;
    let recorded = record_take_observation(&mut tx, observation, now).await?;
    if recorded.is_none() {
        tx.rollback().await?;
        return Ok(false);
    }
    tx.commit().await?;
    Ok(true)
}

/// 端口参数校验：只接受邮箱维护能观察到的原因与合法身份。
pub(super) fn valid_observation(
    delivery_id: Uuid,
    fencing_version: i64,
    reason: SettlementReturnTakeUnknownReason,
    sweep_retries: i64,
    expectation: &DeliveryTakeExpectation,
) -> bool {
    !delivery_id.is_nil()
        && fencing_version > 0
        && reason.observed()
        && (0..=8).contains(&sweep_retries)
        && expectation.validate()
}
