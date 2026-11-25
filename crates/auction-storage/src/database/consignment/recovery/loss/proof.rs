//! 灭失证据的只追加写入：先判证据是否足够，再在锁内推导三个摘要，同事务落库。
//!
//! 保护不变量 1、6、8：空库存、缺 MID、说明文字不构成灭失证据，失败关闭为 `evidence_unproven`；
//! 后续移动链、排除在途与证据摘要只由存储推导（`proof_facts`），推导所需事实不全或不能排除在途 /
//! 已交接同样 `evidence_unproven`；证据一经写入不改不删，同命令 ID 同请求返回原证据、同 ID 异请求为
//! `command_id_conflict`，同来源同观察重放返回原证据，内容矛盾判 `evidence_conflict` 留案。

use auction_application::consignment::{
    ConsignmentError, DepositBusinessRejection, LossLastKnownRef, LossProofReceipt, loss_proof_id,
};
use auction_domain::{CommandId, valid_text};
use auction_time::Timestamp;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::proof_facts::{ProofText, derive_proof_digests};
use crate::database::consignment::commands::errors::WriteError;

/// 合同点名“不足”的三类来源：空库存、缺 MID、说明文字。其余来源是否独立由证据 owner 保证，
/// 合同没有给出独立来源的闭集，故这里只拒绝已点名的不足类别并要求结构完整（见车道报告“与合同的出入”）。
const INSUFFICIENT_SOURCES: [&str; 3] = ["empty_inventory", "missing_mid", "narrative"];

/// 追加灭失证据所需的全部输入；调用方负责命令账本与实物锁。
pub(crate) struct LossProofInput<'a> {
    /// 目标 position。
    pub(crate) position_id: Uuid,
    /// 独立证据来源。
    pub(crate) source_kind: &'a str,
    /// 来源侧的观察身份。
    pub(crate) observation_ref: &'a str,
    /// 最后确定位置（安全文本）。
    pub(crate) last_known_ref: &'a str,
    /// 录入证据的命令。
    pub(crate) command_id: CommandId,
    /// 数据库锁后时刻。
    pub(crate) now: Timestamp,
}

/// 证据文本是否成立：来源不在“不足”类别，三段文本是合法安全文本（长度上限与表 CHECK 的
/// `consignment_valid_text` 一致），观察身份与最后确定位置都不含装备链接与已知凭据形态。
fn evidence_is_sufficient(input: &LossProofInput<'_>) -> bool {
    !INSUFFICIENT_SOURCES.contains(&input.source_kind)
        && valid_text(input.source_kind, 64)
        && valid_text(input.observation_ref, 128)
        && LossLastKnownRef::new(input.observation_ref.to_owned()).is_ok()
        && LossLastKnownRef::new(input.last_known_ref.to_owned()).is_ok()
}

/// 追加一条灭失证据；同命令已有证据时只在请求逐项相同才返回原证据，同来源同观察已存在且内容一致时
/// 也返回原证据，不新增行。证据 ID 由 position、来源与观察身份系统派生。
pub(crate) async fn append_loss_proof(
    connection: &mut PgConnection,
    input: &LossProofInput<'_>,
) -> Result<LossProofReceipt, WriteError> {
    if !evidence_is_sufficient(input) {
        return Err(ConsignmentError::Deposit(DepositBusinessRejection::EvidenceUnproven).into());
    }
    // 不变量 2：共享锁住 position 行，世界与 EID 取自 position 而不是调用方，证据不会挂到别的实物上。
    let position = sqlx::query(
        "SELECT p.world_id, p.canonical_eid, w.season_id
         FROM public.consignment_custody_positions AS p
         LEFT JOIN public.consignment_season_windows AS w ON w.id = p.season_window_id
         WHERE p.id = $1 FOR SHARE OF p",
    )
    .bind(input.position_id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(position) = position else {
        return Err(ConsignmentError::Deposit(DepositBusinessRejection::SourceMismatch).into());
    };
    let world_id: String = position.try_get("world_id")?;
    let canonical_eid: String = position.try_get("canonical_eid")?;
    // 季同样取自 position 绑定的季窗；main 与未绑季窗的 position 为空。
    let season_id: Option<String> = position.try_get("season_id")?;
    let text = ProofText {
        position_id: input.position_id,
        source_kind: input.source_kind,
        observation_ref: input.observation_ref,
        last_known_ref: input.last_known_ref,
    };
    let Some(derived) = derive_proof_digests(connection, &text).await? else {
        return Err(ConsignmentError::Deposit(DepositBusinessRejection::EvidenceUnproven).into());
    };

    // 不变量 1：同命令 ID 只能对应同一请求；账本尚未接入时，原语自己兜底。
    let by_command = sqlx::query(
        "SELECT proof_id, position_id, source_kind, observation_ref, last_known_ref, digest,
                movement_digest, exclusion_digest
         FROM public.consignment_loss_proofs WHERE origin_command_id = $1",
    )
    .bind(input.command_id.as_uuid())
    .fetch_optional(&mut *connection)
    .await?;
    if let Some(row) = by_command {
        let same = row.try_get::<Uuid, _>("position_id")? == input.position_id
            && row.try_get::<String, _>("source_kind")? == input.source_kind
            && row.try_get::<String, _>("observation_ref")? == input.observation_ref
            && row.try_get::<String, _>("last_known_ref")? == input.last_known_ref
            && row.try_get::<Vec<u8>, _>("digest")? == derived.proof
            && row.try_get::<Vec<u8>, _>("movement_digest")? == derived.movement
            && row.try_get::<Vec<u8>, _>("exclusion_digest")? == derived.exclusion;
        if !same {
            return Err(ConsignmentError::CommandIdConflict.into());
        }
        let proof_id: Uuid = row.try_get("proof_id")?;
        return Ok(LossProofReceipt::new(
            input.command_id,
            input.position_id,
            proof_id,
            derived.proof,
        ));
    }

    let existing = sqlx::query(
        "SELECT proof_id, digest, movement_digest, exclusion_digest
         FROM public.consignment_loss_proofs
         WHERE position_id = $1 AND source_kind = $2 AND observation_ref = $3",
    )
    .bind(input.position_id)
    .bind(input.source_kind)
    .bind(input.observation_ref)
    .fetch_optional(&mut *connection)
    .await?;
    if let Some(row) = existing {
        let same = row.try_get::<Vec<u8>, _>("digest")? == derived.proof
            && row.try_get::<Vec<u8>, _>("movement_digest")? == derived.movement
            && row.try_get::<Vec<u8>, _>("exclusion_digest")? == derived.exclusion;
        if !same {
            // 同一远端观察的内容矛盾：不覆盖先前证据，转人工核对。
            return Err(
                ConsignmentError::Deposit(DepositBusinessRejection::EvidenceConflict).into()
            );
        }
        let proof_id: Uuid = row.try_get("proof_id")?;
        return Ok(LossProofReceipt::new(
            input.command_id,
            input.position_id,
            proof_id,
            derived.proof,
        ));
    }

    let proof_id = loss_proof_id(input.position_id, input.source_kind, input.observation_ref);
    sqlx::query(
        "INSERT INTO public.consignment_loss_proofs
            (proof_id, position_id, world_id, canonical_eid, season_id, last_known_ref,
             movement_digest, exclusion_digest, source_kind, observation_ref, digest,
             origin_command_id, recorded_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(proof_id)
    .bind(input.position_id)
    .bind(world_id)
    .bind(canonical_eid)
    .bind(season_id)
    .bind(input.last_known_ref)
    .bind(derived.movement.as_slice())
    .bind(derived.exclusion.as_slice())
    .bind(input.source_kind)
    .bind(input.observation_ref)
    .bind(derived.proof.as_slice())
    .bind(input.command_id.as_uuid())
    .bind(input.now)
    .execute(connection)
    .await?;
    Ok(LossProofReceipt::new(input.command_id, input.position_id, proof_id, derived.proof))
}
