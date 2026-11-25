//! 寄售派发 executor 的义务领取：只领寄售（`contract_id IS NOT NULL`）且最新轮尚待起写的义务，
//! 返回履约 claim、轮键与轮上冻结的身份。
//!
//! 保护不变量 2、5、6：
//! - 与通用拍卖领取同机制、同锁序（先义务后履约行，`FOR UPDATE SKIP LOCKED`，锁后同一谓词复核，
//!   条件更新写 claim 与 fence，短事务先提交释放）；通用领取以 `contract_id IS NULL` 排除寄售行，
//!   本领取以 `contract_id IS NOT NULL` 排除拍卖行，两条路径互不越界。
//! - 只领“有事可做”的义务：最新轮的投递仍在 `prepared` / `pending`，或 `claimed` 但租约已过；
//!   履约行要么仍是初始 `pending` 且不指向投递，要么已指向这一轮的投递（衔接只会推进，绝不改指）。
//!   已起写、已终结或结果未知的投递不会再被领取，executor 不会对它们空转。
//! - 轮上冻结身份与投递载荷的身份摘要不一致说明来源被改坏：转人工对账、不派发、不自动重试。
//! - 只锁义务与履约行（先义务后履约、`SKIP LOCKED`）；合同、轮与投递只读。起写的最终事务从实物
//!   锁序起点重新取锁（D9 §1.3）。

use auction_application::consignment::{ConsignmentError, DispatchWorkClaim};
use auction_time::Timestamp;
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use super::first_dispatch::{payload_equipment_ids, payload_matches};
use super::round::load_by_delivery;
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::settlement::fulfillment::manual_reconcile_in_transaction;

/// 一次最多锁定的候选数；只领取其中第一个可领的，其余在提交时释放留给下一次。
const CANDIDATES: i64 = 16;

/// 可领取的寄售派发义务谓词（`$1` 为当前时刻）；别名 `o` 义务、`f` 履约行、`prerequisite`
/// 前置履约行、`d` 最新轮的投递。
macro_rules! dispatchable {
    () => {
        "o.contract_id IS NOT NULL
    AND o.business_state = 'ready'
    AND f.execution_state IN ('pending', 'prepared', 'enqueued')
    AND ((f.claim_token IS NULL AND (f.next_attempt_at IS NULL OR f.next_attempt_at <= $1))
         OR f.claim_expires_at <= $1)
    AND (o.prerequisite_obligation_id IS NULL OR prerequisite.execution_state = 'satisfied')
    AND (d.status IN ('prepared', 'pending')
         OR (d.status = 'claimed' AND d.lease_expires_at <= $1))
    AND ((f.delivery_id IS NULL AND f.execution_state = 'pending') OR f.delivery_id = d.id)"
    };
}

/// 义务、履约行、前置与最新轮投递的连接；最新轮取该义务轮号最大的一轮。
macro_rules! dispatch_sources {
    () => {
        "public.settlement_obligations AS o
     JOIN public.settlement_obligation_fulfillments AS f ON f.obligation_id = o.id
     LEFT JOIN public.settlement_obligation_fulfillments AS prerequisite
         ON prerequisite.obligation_id = o.prerequisite_obligation_id
     JOIN LATERAL (
         SELECT latest.delivery_id FROM public.consignment_dispatch_rounds AS latest
         WHERE latest.contract_id = o.contract_id AND latest.obligation_id = o.id
         ORDER BY latest.round DESC LIMIT 1
     ) AS round_row ON true
     JOIN public.deliveries AS d ON d.id = round_row.delivery_id"
    };
}

impl DatabaseStore {
    /// 领取一项待派发的寄售义务（见模块说明）；没有时返回 `None`。
    pub(super) async fn claim_dispatch_obligation(
        &self,
        now: Timestamp,
        lease_until: Timestamp,
    ) -> Result<Option<DispatchWorkClaim>, WriteError> {
        let mut transaction = self.pool().begin().await?;
        // 先按创建顺序截取有界候选，再按 UUID 锁义务；被他人锁住的候选直接跳过。
        let obligation_ids = sqlx::query_scalar::<_, Uuid>(concat!(
            "WITH candidates AS MATERIALIZED (
                 SELECT o.id FROM ",
            dispatch_sources!(),
            "
                 WHERE ",
            dispatchable!(),
            "
                 ORDER BY o.created_at, o.id
                 LIMIT $2
             )
             SELECT o.id FROM public.settlement_obligations AS o
             JOIN candidates ON candidates.id = o.id
             WHERE o.business_state = 'ready' AND o.contract_id IS NOT NULL
             ORDER BY o.id FOR UPDATE OF o SKIP LOCKED"
        ))
        .bind(now)
        .bind(CANDIDATES)
        .fetch_all(&mut *transaction)
        .await?;

        // 再锁履约行（先义务后履约，与拍卖领取同锁序）；锁后用同一谓词复核。
        let rows = sqlx::query(concat!(
            "SELECT f.obligation_id, f.execution_state, f.claim_fencing, f.state_version,
                    d.id AS delivery_id, d.payload
             FROM ",
            dispatch_sources!(),
            "
             WHERE o.id = ANY($2) AND ",
            dispatchable!(),
            "
             ORDER BY f.obligation_id
             FOR UPDATE OF f SKIP LOCKED"
        ))
        .bind(now)
        .bind(&obligation_ids)
        .fetch_all(&mut *transaction)
        .await?;

        let mut claimed = None;
        for row in rows {
            let obligation_id: Uuid = row.try_get("obligation_id")?;
            let execution_state: String = row.try_get("execution_state")?;
            let old_fencing: i64 = row.try_get("claim_fencing")?;
            let state_version: i64 = row.try_get("state_version")?;
            let delivery_id: Uuid = row.try_get("delivery_id")?;
            let payload: Value = row.try_get("payload")?;
            let (Some(claim_fencing), Some(next_state_version)) = (
                old_fencing.checked_add(1),
                state_version.checked_add(1).and_then(|value| u64::try_from(value).ok()),
            ) else {
                return Err(WriteError::invariant("consignment_fulfillment_counters"));
            };
            let claim_token = Uuid::new_v4();
            // 条件更新：只接管空闲或已过期的 claim；并发领取者在行锁后看到新 claim 即放弃。
            let updated = sqlx::query(
                "UPDATE public.settlement_obligation_fulfillments
                 SET claim_token = $2, claimed_at = $3, claim_expires_at = $4, claim_fencing = $5,
                     state_version = state_version + 1, next_attempt_at = NULL,
                     updated_at = statement_timestamp()
                 WHERE obligation_id = $1
                   AND execution_state IN ('pending', 'prepared', 'enqueued')
                   AND ((claim_token IS NULL AND (next_attempt_at IS NULL OR next_attempt_at <= $3))
                        OR claim_expires_at <= $3)",
            )
            .bind(obligation_id)
            .bind(claim_token)
            .bind(now)
            .bind(lease_until)
            .bind(claim_fencing)
            .execute(&mut *transaction)
            .await?;
            if updated.rows_affected() != 1 {
                continue;
            }
            let round = load_by_delivery(&mut transaction, delivery_id)
                .await?
                .ok_or_else(|| WriteError::invariant("consignment_dispatch_round_missing"))?;
            let frozen = payload_equipment_ids(&payload)
                .map(|equipment_ids| round.frozen(equipment_ids))
                .transpose()?
                .filter(|frozen| payload_matches(&payload, frozen));
            let Some(frozen) = frozen else {
                // 不变量 6：轮与投递载荷不是同一封，不能派发；转人工对账、不自动重试。
                manual_reconcile_in_transaction(
                    &mut transaction,
                    obligation_id,
                    &execution_state,
                    next_state_version,
                    claim_token,
                    claim_fencing,
                    "consignment_dispatch_identity_mismatch",
                )
                .await
                .map_err(|error| {
                    tracing::warn!(%error, "寄售派发来源不一致转人工失败");
                    WriteError::from(ConsignmentError::Unavailable)
                })?;
                continue;
            };
            claimed = Some(DispatchWorkClaim {
                obligation_id,
                claim_token,
                claim_fencing,
                key: round.key,
                frozen,
            });
            break;
        }
        transaction.commit().await?;
        Ok(claimed)
    }
}
