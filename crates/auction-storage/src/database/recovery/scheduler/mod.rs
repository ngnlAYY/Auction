//! PostgreSQL recovery case 到期推进适配器；只锁定 case、观察已持久化的结算履约事实并创建下一条
//! 可审计 action，不读外部凭据、不直接发 Mail，外部动作须在 delivery owner 提交后执行。

use auction_application::auction::ApplicationFuture;
use auction_application::core::{ApplicationError, RequestHash};
use auction_application::recovery::{
    RecoveryActionClaim, RecoveryActionOutcome, RecoveryCaseAdvance, RecoveryReturnAdvance,
    RecoveryWorkStore,
};
use auction_domain::CommandId;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use self::actions::{
    claim_action_postgres, link_action_postgres, manual_action_postgres,
    mark_action_enqueued_postgres, mark_action_satisfied_postgres, observe_action_postgres,
    release_action_postgres,
};
use self::advance::advance_postgres;
use self::mapping::valid_failure_code;
use self::returns::record_return_postgres;
use crate::database::DatabaseStore;
mod actions;
mod advance;
mod mapping;
mod returns;

impl RecoveryWorkStore for DatabaseStore {
    fn record_due_return(
        &self,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, Option<RecoveryReturnAdvance>> {
        Box::pin(async move {
            if now.timestamp_nanos_opt().is_none() {
                return Err(repository_error("recovery 退回邮件记录时间无效").into());
            }
            record_return_postgres(self.pool(), now).await.map_err(ApplicationError::from)
        })
    }

    fn advance_due_case(
        &self,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, Option<RecoveryCaseAdvance>> {
        Box::pin(async move {
            if now.timestamp_nanos_opt().is_none() {
                return Err(repository_error("recovery Worker 时间无效").into());
            }
            advance_postgres(self.pool(), now).await.map_err(ApplicationError::from)
        })
    }

    fn claim_due_action(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
    ) -> ApplicationFuture<'_, Option<RecoveryActionClaim>> {
        Box::pin(async move {
            if worker_id.is_empty()
                || worker_id != worker_id.trim()
                || now.timestamp_nanos_opt().is_none()
                || lease_until <= now
            {
                return Err(repository_error("recovery action 领取参数无效").into());
            }
            claim_action_postgres(self.pool(), &worker_id, now, lease_until)
                .await
                .map_err(ApplicationError::from)
        })
    }

    fn link_action_preparation(
        &self,
        claim: RecoveryActionClaim,
        delivery_id: Uuid,
        command_id: CommandId,
        request_hash: RequestHash,
        linked_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, RecoveryActionClaim> {
        Box::pin(async move {
            if claim.action_id.is_nil()
                || claim.recovery_id.is_nil()
                || claim.execution_state != "pending"
                || claim.claim_token.is_nil()
                || delivery_id.is_nil()
                || command_id.is_nil()
                || request_hash.as_bytes() == [0; 32]
                || linked_at.timestamp_nanos_opt().is_none()
            {
                return Err(repository_error("recovery action prepared 关联参数无效").into());
            }
            link_action_postgres(
                self.pool(),
                &claim,
                delivery_id,
                command_id,
                request_hash,
                linked_at,
            )
            .await
            .map_err(ApplicationError::from)
        })
    }

    fn mark_action_enqueued(
        &self,
        claim: RecoveryActionClaim,
        activate_command_id: CommandId,
        activate_request_hash: RequestHash,
        activated_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, RecoveryActionClaim> {
        Box::pin(async move {
            if claim.action_id.is_nil()
                || claim.recovery_id.is_nil()
                || claim.execution_state != "prepared"
                || claim.claim_token.is_nil()
                || claim.delivery_id.is_none_or(|value| value.is_nil())
                || claim.prepare_command_id.is_none_or(CommandId::is_nil)
                || claim.prepare_request_hash.is_none_or(|value| value.as_bytes() == [0; 32])
                || activate_command_id.is_nil()
                || activate_request_hash.as_bytes() == [0; 32]
                || activated_at.timestamp_nanos_opt().is_none()
            {
                return Err(repository_error("recovery action activation 参数无效").into());
            }
            mark_action_enqueued_postgres(
                self.pool(),
                &claim,
                activate_command_id,
                activate_request_hash,
                activated_at,
            )
            .await
            .map_err(ApplicationError::from)
        })
    }

    fn mark_action_satisfied(
        &self,
        claim: RecoveryActionClaim,
        proof_kind: String,
        proof_reference: String,
        proof_digest: [u8; 32],
        observed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if claim.action_id.is_nil()
                || claim.recovery_id.is_nil()
                || claim.buyer_user_id.is_nil()
                || claim.kind != "buyer_blacklist"
                || claim.execution_state != "pending"
                || claim.delivery_id.is_some()
                || claim.claim_token.is_nil()
                || observed_at < claim.claimed_at
                || observed_at > claim.lease_until
                || observed_at.timestamp_nanos_opt().is_none()
                || !valid_failure_code(&proof_kind)
                || !valid_failure_code(&proof_reference)
                || proof_digest == [0; 32]
            {
                return Err(repository_error("recovery action 完成证明参数无效").into());
            }
            mark_action_satisfied_postgres(
                self.pool(),
                &claim,
                &proof_kind,
                &proof_reference,
                proof_digest,
                observed_at,
            )
            .await
            .map_err(ApplicationError::from)
        })
    }

    fn observe_action(
        &self,
        claim: RecoveryActionClaim,
        observed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, RecoveryActionOutcome> {
        Box::pin(async move {
            if claim.action_id.is_nil()
                || claim.recovery_id.is_nil()
                || claim.execution_state != "enqueued"
                || claim.claim_token.is_nil()
                || claim.delivery_id.is_none_or(|value| value.is_nil())
                || observed_at < claim.claimed_at
                || observed_at.timestamp_nanos_opt().is_none()
            {
                return Err(repository_error("recovery action 观察参数无效").into());
            }
            observe_action_postgres(self.pool(), &claim, observed_at)
                .await
                .map_err(ApplicationError::from)
        })
    }

    fn release_action(
        &self,
        claim: RecoveryActionClaim,
        retry_at: DateTime<Utc>,
        failure_code: String,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if claim.action_id.is_nil()
                || claim.recovery_id.is_nil()
                || claim.claim_token.is_nil()
                || retry_at.timestamp_nanos_opt().is_none()
                || !valid_failure_code(&failure_code)
            {
                return Err(repository_error("recovery action 释放参数无效").into());
            }
            release_action_postgres(self.pool(), &claim, retry_at, &failure_code)
                .await
                .map_err(ApplicationError::from)
        })
    }

    fn mark_action_manual_reconcile(
        &self,
        claim: RecoveryActionClaim,
        failure_code: String,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if claim.action_id.is_nil()
                || claim.recovery_id.is_nil()
                || claim.claim_token.is_nil()
                || !valid_failure_code(&failure_code)
            {
                return Err(repository_error("recovery action 人工复核参数无效").into());
            }
            manual_action_postgres(self.pool(), &claim, &failure_code)
                .await
                .map_err(ApplicationError::from)
        })
    }
}

pub(crate) use crate::database::repository_error;
