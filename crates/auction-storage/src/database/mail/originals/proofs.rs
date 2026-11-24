//! 资产起写与邮件证明的持久事实：唯一成功起写、原 attempt/fence 与证明结论。

use auction_application::consignment::{
    DepositBusinessRejection, DispositionAction, MailProofKind, ProofDisposition, ProofReceipt,
};
use auction_domain::CommandId;
use auction_time::Timestamp;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::{conflict, invariant, invariant_decoding};
use crate::database::consignment::commands::errors::WriteError;

/// 资产工作行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkRow {
    /// 工作 UUID。
    pub(crate) work_id: Uuid,
    /// 决定 UUID。
    pub(crate) decision_id: Uuid,
    /// 投递 UUID。
    pub(crate) delivery_id: Uuid,
    /// 动作。
    pub(crate) action: DispositionAction,
    /// 预期观察 UUID。
    pub(crate) expected_observation_id: Uuid,
    /// 冻结清单摘要。
    pub(crate) manifest_digest: [u8; 32],
}

/// 读取并锁定工作行。
pub(crate) async fn load_work_for_update(
    connection: &mut PgConnection,
    work_id: Uuid,
) -> Result<Option<WorkRow>, WriteError> {
    let row = sqlx::query(
        "SELECT id, decision_id, delivery_id, action, expected_observation_id, manifest_digest
         FROM public.mail_disposition_work WHERE id = $1",
    )
    .bind(work_id)
    .fetch_optional(connection)
    .await?;
    row.map(|row| {
        let digest: &[u8] = row.try_get("manifest_digest")?;
        Ok(WorkRow {
            work_id: row.try_get("id")?,
            decision_id: row.try_get("decision_id")?,
            delivery_id: row.try_get("delivery_id")?,
            action: action(row.try_get("action")?)?,
            expected_observation_id: row.try_get("expected_observation_id")?,
            manifest_digest: <[u8; 32]>::try_from(digest)
                .map_err(invariant_decoding("consignment_work_manifest_digest"))?,
        })
    })
    .transpose()
}

fn action(value: &str) -> Result<DispositionAction, WriteError> {
    match value {
        "take" => Ok(DispositionAction::Take),
        "return" => Ok(DispositionAction::Return),
        _ => Err(invariant("consignment_work_action")),
    }
}

/// 已成功起写事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StartRow {
    /// 原命令 UUID。
    pub(crate) command_id: Uuid,
    /// 决定 UUID。
    pub(crate) decision_id: Option<Uuid>,
    /// 投递 UUID。
    pub(crate) delivery_id: Uuid,
    /// attempt UUID。
    pub(crate) attempt_id: Uuid,
    /// fence。
    pub(crate) fence: u64,
}

/// 读取该工作的唯一成功起写。
pub(crate) async fn load_start(
    connection: &mut PgConnection,
    work_id: Uuid,
) -> Result<Option<StartRow>, WriteError> {
    let row = sqlx::query(
        "SELECT command_id, decision_id, delivery_id, attempt_id, fence
         FROM public.mail_disposition_write_starts WHERE work_id = $1",
    )
    .bind(work_id)
    .fetch_optional(connection)
    .await?;
    row.map(|row| {
        Ok(StartRow {
            command_id: row.try_get("command_id")?,
            decision_id: row.try_get("decision_id")?,
            delivery_id: row.try_get("delivery_id")?,
            attempt_id: row.try_get("attempt_id")?,
            fence: u64::try_from(row.try_get::<i64, _>("fence")?)
                .map_err(invariant_decoding("consignment_start_fence"))?,
        })
    })
    .transpose()
}

/// 插入首次成功起写；唯一 `work_id` 保证同一工作只有一份 POST 许可。
#[allow(clippy::too_many_arguments, reason = "逐列复刻唯一成功起写事实")]
pub(crate) async fn insert_start(
    connection: &mut PgConnection,
    work_id: Uuid,
    decision_id: Uuid,
    delivery_id: Uuid,
    attempt_id: Uuid,
    fence: u64,
    command_id: Uuid,
    now: Timestamp,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO public.mail_disposition_write_starts
            (work_id, decision_id, delivery_id, attempt_id, fence, started_at, command_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(work_id)
    .bind(decision_id)
    .bind(delivery_id)
    .bind(attempt_id)
    .bind(i64::try_from(fence)?)
    .bind(now)
    .bind(command_id)
    .execute(connection)
    .await?;
    Ok(())
}

/// 投递尝试的安全绑定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttemptRow {
    /// attempt UUID。
    pub(crate) attempt_id: Uuid,
    /// 工作 UUID。
    pub(crate) work_id: Uuid,
    /// 投递 UUID。
    pub(crate) delivery_id: Uuid,
    /// fence。
    pub(crate) fence: u64,
    /// 会话 UUID。
    pub(crate) session_id: Uuid,
    /// 预期清单摘要。
    pub(crate) expected_digest: [u8; 32],
}

/// 读取原 attempt。
pub(crate) async fn load_attempt(
    connection: &mut PgConnection,
    attempt_id: Uuid,
) -> Result<Option<AttemptRow>, WriteError> {
    let row = sqlx::query(
        "SELECT id, work_id, delivery_id, fence, session_id, expected_digest
         FROM public.mail_disposition_attempts WHERE id = $1",
    )
    .bind(attempt_id)
    .fetch_optional(connection)
    .await?;
    row.map(|row| {
        let digest: &[u8] = row.try_get("expected_digest")?;
        Ok(AttemptRow {
            attempt_id: row.try_get("id")?,
            work_id: row.try_get("work_id")?,
            delivery_id: row.try_get("delivery_id")?,
            fence: u64::try_from(row.try_get::<i64, _>("fence")?)
                .map_err(invariant_decoding("consignment_attempt_fence"))?,
            session_id: row.try_get("session_id")?,
            expected_digest: <[u8; 32]>::try_from(digest)
                .map_err(invariant_decoding("consignment_attempt_digest"))?,
        })
    })
    .transpose()
}

/// 证明写入参数。
pub(crate) struct ProofWrite<'a> {
    /// 证明 UUID。
    pub(crate) proof_id: Uuid,
    /// 工作 UUID。
    pub(crate) work_id: Uuid,
    /// 决定 UUID。
    pub(crate) decision_id: Uuid,
    /// 投递 UUID。
    pub(crate) delivery_id: Uuid,
    /// 原 fence。
    pub(crate) fence: u64,
    /// 原 attempt UUID。
    pub(crate) attempt_id: Uuid,
    /// 证明种类。
    pub(crate) kind: MailProofKind,
    /// 结论。
    pub(crate) conclusion: ProofDisposition,
    /// 观察 UUID。
    pub(crate) observation_id: Uuid,
    /// 验证器版本。
    pub(crate) validator_version: &'a str,
    /// 证明摘要。
    pub(crate) digest: [u8; 32],
    /// 证明时刻。
    pub(crate) now: Timestamp,
}

/// 追加一条证明；结论性证明占唯一槽，待核证明可多条且只追加。
pub(crate) async fn upsert_proof(
    connection: &mut PgConnection,
    proof: &ProofWrite<'_>,
) -> Result<(Uuid, ProofDisposition), WriteError> {
    let conclusion = proof.conclusion;
    let kind = proof.kind.as_str();
    let existing = sqlx::query(
        "SELECT id, conclusion, digest, observation_id
         FROM public.mail_disposition_proofs
         WHERE work_id = $1 AND asset_leg = 'original'
         ORDER BY CASE WHEN conclusion = 'evidence_quarantined' THEN 1 ELSE 0 END,
                  proven_at DESC, id DESC
         LIMIT 1",
    )
    .bind(proof.work_id)
    .fetch_optional(&mut *connection)
    .await?;
    if let Some(row) = existing {
        let stored: String = row.try_get("conclusion")?;
        let id: Uuid = row.try_get("id")?;
        let stored = conclusion_of(&stored)?;
        if stored != ProofDisposition::EvidenceQuarantined {
            if stored != conclusion {
                return Err(conflict(DepositBusinessRejection::EvidenceConflict));
            }
            return Ok((id, stored));
        }
        if conclusion == ProofDisposition::EvidenceQuarantined {
            let digest: &[u8] = row.try_get("digest")?;
            let observation_id: Uuid = row.try_get("observation_id")?;
            if digest == proof.digest.as_slice() && observation_id == proof.observation_id {
                return Ok((id, stored));
            }
        }
    }
    sqlx::query(
        "INSERT INTO public.mail_disposition_proofs
            (id, work_id, decision_id, delivery_id, fence, attempt_id, kind, asset_leg,
             observation_id, validator_version, digest, conclusion, proven_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'original', $8, $9, $10, $11, $12)",
    )
    .bind(proof.proof_id)
    .bind(proof.work_id)
    .bind(proof.decision_id)
    .bind(proof.delivery_id)
    .bind(i64::try_from(proof.fence)?)
    .bind(proof.attempt_id)
    .bind(kind)
    .bind(proof.observation_id)
    .bind(proof.validator_version)
    .bind(proof.digest.as_slice())
    .bind(conclusion.as_str())
    .bind(proof.now)
    .execute(connection)
    .await?;
    Ok((proof.proof_id, conclusion))
}

fn conclusion_of(value: &str) -> Result<ProofDisposition, WriteError> {
    match value {
        "take_proven" => Ok(ProofDisposition::TakeProven),
        "return_proven" => Ok(ProofDisposition::ReturnProven),
        "evidence_quarantined" => Ok(ProofDisposition::EvidenceQuarantined),
        _ => Err(invariant("consignment_proof_conclusion")),
    }
}

/// 读取已有证明回执；用于同一工作不同命令的相容重放。
pub(crate) async fn existing_proof_receipt(
    connection: &mut PgConnection,
    work_id: Uuid,
    command_id: CommandId,
    decision_id: Uuid,
) -> Result<Option<ProofReceipt>, WriteError> {
    let row = sqlx::query(
        "SELECT id, conclusion FROM public.mail_disposition_proofs
         WHERE work_id = $1 AND asset_leg = 'original'
         ORDER BY CASE WHEN conclusion = 'evidence_quarantined' THEN 1 ELSE 0 END,
                  proven_at DESC, id DESC
         LIMIT 1",
    )
    .bind(work_id)
    .fetch_optional(connection)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let conclusion: String = row.try_get("conclusion")?;
    let disposition = match conclusion.as_str() {
        "take_proven" => ProofDisposition::TakeProven,
        "return_proven" => ProofDisposition::ReturnProven,
        "evidence_quarantined" => ProofDisposition::EvidenceQuarantined,
        _ => return Err(invariant("consignment_proof_conclusion")),
    };
    Ok(Some(ProofReceipt::new(command_id, disposition, decision_id, row.try_get("id")?)))
}

/// 读取证明摘要与观察，供相容性比较。
pub(crate) async fn load_proof_identity(
    connection: &mut PgConnection,
    work_id: Uuid,
) -> Result<Option<([u8; 32], Uuid)>, WriteError> {
    let row = sqlx::query(
        "SELECT digest, observation_id FROM public.mail_disposition_proofs
         WHERE work_id = $1 AND asset_leg = 'original'
         ORDER BY CASE WHEN conclusion = 'evidence_quarantined' THEN 1 ELSE 0 END,
                  proven_at DESC, id DESC
         LIMIT 1",
    )
    .bind(work_id)
    .fetch_optional(connection)
    .await?;
    row.map(|row| {
        let digest: &[u8] = row.try_get("digest")?;
        Ok((
            <[u8; 32]>::try_from(digest).map_err(invariant_decoding("consignment_proof_digest"))?,
            row.try_get("observation_id")?,
        ))
    })
    .transpose()
}
