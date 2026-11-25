//! 损失案件的只读视图：卖家本人按 position 查回执，Admin 按 position 查案件与全部人工结果。
//!
//! 只读原语不写任何表；卖家视图按卖家过滤，非本人与不存在不可区分（调用方统一 404）。
//! 视图不含装备 key、URL、证据原文或任何金额字段（不变量 6、7）。

use auction_time::Timestamp;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use crate::database::consignment::commands::errors::WriteError;
use crate::database::pool::TrackedPool;

/// 卖家可查的损失回执：当前案件状态与最近一份回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SellerLossCaseView {
    /// position UUID。
    pub position_id: Uuid,
    /// 案件 UUID。
    pub loss_case_id: Uuid,
    /// 当前状态：`opened`、`reviewing` 或 `closed`。
    pub state: String,
    /// 最近一次回执的记录时刻。
    pub recorded_at: Timestamp,
    /// 最近一份卖家回执 UUID。
    pub receipt_id: Uuid,
}

/// 案件的一条追加记录（开案或人工结果）。
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    not(feature = "test-db"),
    allow(dead_code, reason = "只供 test-db 测试入口读全量记录；生产 Admin 读口在 loss::admin")
)]
pub struct LossCaseOutcomeView {
    /// 结果记录 UUID。
    pub outcome_id: Uuid,
    /// 该记录对应的案件版本。
    pub case_version: u64,
    /// `opened`、`reviewing` 或 `closed`。
    pub outcome: String,
    /// 依据 UUID；开案记录为空。
    pub basis_id: Option<Uuid>,
    /// 私有说明；开案记录为空。
    pub private_note: Option<String>,
    /// 记录时刻。
    pub occurred_at: Timestamp,
    /// 随该记录写下的卖家回执 UUID。
    pub seller_receipt_id: Uuid,
}

/// Admin 的损失案件视图；证据只给来源与摘要，不给原文。
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    not(feature = "test-db"),
    allow(dead_code, reason = "只供 test-db 测试入口读全量记录；生产 Admin 读口在 loss::admin")
)]
pub struct AdminLossCaseView {
    /// position UUID。
    pub position_id: Uuid,
    /// 案件 UUID。
    pub case_id: Uuid,
    /// 唯一损失事实 UUID。
    pub loss_fact_id: Uuid,
    /// 支撑损失事实的证据 UUID。
    pub proof_id: Uuid,
    /// 证据来源。
    pub proof_source_kind: String,
    /// 证据摘要。
    pub proof_digest: [u8; 32],
    /// 挂单 UUID。
    pub listing_id: Uuid,
    /// 卖家本地用户 UUID。
    pub seller_user_id: Uuid,
    /// 确认灭失时的处分代；指针不因损失改变。
    pub generation: u64,
    /// 最新案件版本。
    pub case_version: u64,
    /// 当前状态。
    pub state: String,
    /// 按版本升序的全部记录。
    pub outcomes: Vec<LossCaseOutcomeView>,
}

/// 卖家按 position 查本人的损失案件；非本人或没有案件返回 `None`。
pub(crate) async fn seller_loss_case(
    pool: &TrackedPool<Postgres>,
    seller_user_id: Uuid,
    position_id: Uuid,
) -> Result<Option<SellerLossCaseView>, WriteError> {
    let row = sqlx::query(
        "SELECT c.position_id, c.case_id, o.outcome, r.recorded_at, r.receipt_id
         FROM public.consignment_loss_cases AS c
         JOIN LATERAL (
             SELECT outcome_id, outcome FROM public.consignment_loss_case_outcomes
             WHERE case_id = c.case_id ORDER BY case_version DESC LIMIT 1
         ) AS o ON true
         JOIN public.consignment_loss_receipts AS r ON r.outcome_id = o.outcome_id
         WHERE c.position_id = $1 AND c.seller_user_id = $2",
    )
    .bind(position_id)
    .bind(seller_user_id)
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        Ok(SellerLossCaseView {
            position_id: row.try_get("position_id")?,
            loss_case_id: row.try_get("case_id")?,
            state: row.try_get("outcome")?,
            recorded_at: row.try_get("recorded_at")?,
            receipt_id: row.try_get("receipt_id")?,
        })
    })
    .transpose()
}

/// Admin 按 position 查损失案件与全部追加记录；没有案件返回 `None`。
#[cfg_attr(
    not(feature = "test-db"),
    allow(dead_code, reason = "只供 test-db 测试入口读全量记录；生产 Admin 读口在 loss::admin")
)]
pub(crate) async fn admin_loss_case(
    pool: &TrackedPool<Postgres>,
    position_id: Uuid,
) -> Result<Option<AdminLossCaseView>, WriteError> {
    let header = sqlx::query(
        "SELECT c.case_id, c.loss_fact_id, c.seller_user_id, f.proof_id, f.listing_id, f.generation,
                p.source_kind, p.digest
         FROM public.consignment_loss_cases AS c
         JOIN public.consignment_loss_facts AS f ON f.loss_fact_id = c.loss_fact_id
         JOIN public.consignment_loss_proofs AS p ON p.proof_id = f.proof_id
         WHERE c.position_id = $1",
    )
    .bind(position_id)
    .fetch_optional(pool)
    .await?;
    let Some(header) = header else {
        return Ok(None);
    };
    let case_id: Uuid = header.try_get("case_id")?;
    let rows = sqlx::query(
        "SELECT o.outcome_id, o.case_version, o.outcome, o.basis_id, o.private_note,
                o.occurred_at, r.receipt_id
         FROM public.consignment_loss_case_outcomes AS o
         JOIN public.consignment_loss_receipts AS r ON r.outcome_id = o.outcome_id
         WHERE o.case_id = $1 ORDER BY o.case_version",
    )
    .bind(case_id)
    .fetch_all(pool)
    .await?;
    let outcomes = rows
        .iter()
        .map(|row| {
            Ok(LossCaseOutcomeView {
                outcome_id: row.try_get("outcome_id")?,
                case_version: row.try_get::<i64, _>("case_version")?.cast_unsigned(),
                outcome: row.try_get("outcome")?,
                basis_id: row.try_get("basis_id")?,
                private_note: row.try_get("private_note")?,
                occurred_at: row.try_get("occurred_at")?,
                seller_receipt_id: row.try_get("receipt_id")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let latest = outcomes
        .last()
        .ok_or_else(|| WriteError::invariant("consignment_loss_case_without_outcome"))?;
    let proof_digest: Vec<u8> = header.try_get("digest")?;
    Ok(Some(AdminLossCaseView {
        position_id,
        case_id,
        loss_fact_id: header.try_get("loss_fact_id")?,
        proof_id: header.try_get("proof_id")?,
        proof_source_kind: header.try_get("source_kind")?,
        proof_digest: <[u8; 32]>::try_from(proof_digest.as_slice())
            .ok()
            .ok_or_else(|| WriteError::invariant("consignment_loss_proof_digest"))?,
        listing_id: header.try_get("listing_id")?,
        seller_user_id: header.try_get("seller_user_id")?,
        generation: header.try_get::<i64, _>("generation")?.cast_unsigned(),
        case_version: latest.case_version,
        state: latest.outcome.clone(),
        outcomes,
    }))
}
