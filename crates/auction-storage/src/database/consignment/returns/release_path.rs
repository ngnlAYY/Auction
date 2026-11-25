//! 隔离解除为 `acquire_return` 的同事务接线：复用直接接管的锁内写端，再写解除行与隔离终态。
//!
//! 保护不变量 3、4、6：解除、K 的终局处置、intake 与唯一 Take 工作同一顶层事务提交；
//! 来源只由 RTS 正文末行的派发引用（轮 ID）指认，读不出就继续隔离，不凭 Admin 的拟定放行。

use auction_application::consignment::{DepositBusinessRejection, ReleaseReceipt};
use auction_domain::consignment::deposit::ReleaseKind;
use auction_time::Timestamp;
use uuid::Uuid;

use super::acquire::{AcquireOutcome, AcquireRequest, acquire_in_tx};
use super::intake::RoundLocator;
use crate::database::consignment::commands::DepositWriter;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::mail::originals::identity::StoredQuarantine;
use crate::database::mail::originals::observations::StoredObservation;
use crate::database::mail::originals::{Outcome, conflict};

/// 在解除事务内接管：成功时写解除行、推进隔离状态并以同一回执收口。
pub(crate) async fn release_acquire(
    mut writer: DepositWriter<'_, '_, '_>,
    quarantine: &StoredQuarantine,
    observation: &StoredObservation,
    now: Timestamp,
) -> Result<Outcome<ReleaseReceipt>, WriteError> {
    let Some(round_id) = observation
        .legacy_return_evidence
        .dispatch_reference
        .as_deref()
        .and_then(|reference| Uuid::parse_str(reference).ok())
    else {
        return Err(conflict(DepositBusinessRejection::EvidenceUnproven));
    };
    let command_id = writer.command_id();
    let outcome = acquire_in_tx(
        writer.connection(),
        &AcquireRequest {
            key: &quarantine.mail_key,
            observation_id: observation.observation_id,
            locator: RoundLocator::ByRoundId(round_id),
            command_id,
            write_initial_decision: false,
        },
    )
    .await?;
    let acquired = match outcome {
        AcquireOutcome::Acquired(acquired) => acquired,
        // 隔离解除只处理尚无接管的 K；已有接管走上游的“已有解除”重放，这里出现即矛盾。
        AcquireOutcome::Replay(_) | AcquireOutcome::Conflict(_) => {
            return Err(conflict(DepositBusinessRejection::DispositionConflict));
        }
        AcquireOutcome::Quarantine(reason) => {
            tracing::info!(?reason, "隔离解除为接管被拒，继续隔离");
            return Err(conflict(DepositBusinessRejection::EvidenceUnproven));
        }
    };
    let release_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.mail_quarantine_releases
            (id, bot_uid, world_id, external_mid, quarantine_id, expected_version,
             observation_id, decision_id, kind, command_id, case_return_mail_id,
             case_evidence_id, recovery_handoff_id, released_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'acquire_return', $9, NULL, NULL, NULL, $10)",
    )
    .bind(release_id)
    .bind(quarantine.mail_key.authenticated_bot_uid())
    .bind(auction_domain::consignment::settlement_world_code(quarantine.mail_key.world()))
    .bind(quarantine.mail_key.external_mid())
    .bind(quarantine.quarantine_id)
    .bind(i64::try_from(quarantine.version)?)
    .bind(observation.observation_id)
    .bind(acquired.decision_id)
    .bind(command_id)
    .bind(now)
    .execute(writer.connection())
    .await?;
    let released = sqlx::query(
        "UPDATE public.mail_quarantines
         SET state = 'released', version = version + 1, updated_at = $2
         WHERE id = $1 AND state = 'active' AND version = $3",
    )
    .bind(quarantine.quarantine_id)
    .bind(now)
    .bind(i64::try_from(quarantine.version)?)
    .execute(writer.connection())
    .await?;
    if released.rows_affected() != 1 {
        return Err(conflict(DepositBusinessRejection::DispositionConflict));
    }
    let mut receipt = ReleaseReceipt::released(
        auction_domain::CommandId::from_uuid(command_id),
        quarantine.quarantine_id,
        release_id,
        acquired.decision_id,
        ReleaseKind::AcquireReturn,
    );
    receipt.return_intake_id = Some(acquired.intake_id);
    receipt.original_round = Some(acquired.facts.original_round);
    receipt.take_work_id = Some(acquired.take_work_id);
    receipt.take_delivery_id = Some(acquired.take_delivery_id);
    let _ = writer.finish(receipt.clone(), now).await?;
    Ok(Outcome::Accepted(receipt))
}
