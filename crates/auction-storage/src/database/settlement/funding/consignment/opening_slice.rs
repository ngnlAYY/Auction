//! opening 份额归属：登记（封存逐笔依据）与收款认证时的消费校验。
//!
//! 保护不变量 1、6：每个实收业务键对应一组不可变份额；同一 entry 的区间不相交、每段至多被一个
//! 实收业务键消费一次；份额必须落在本 UID opening 发行分录的正额范围内，且原余额仍在、无他人权利。
//! 证不清、已花、无法逐笔解释一律失败关闭（待核），不凭观察余额或操作员填写的 A 放行。

use std::collections::BTreeMap;

use auction_application::settlement::{
    AttributionReceipt, FundingError, RegisterOpeningAttribution,
};
use auction_domain::consignment::funding::{
    FundingRuleError, OpeningSlice, opening_included_total,
};
use serde_json::json;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::command_ledger::{SuccessReceipt, append_success};
use super::receipt::{funding_sql_error, receipt_key_id, record_replayed_command};
use super::serializable::run_serializable;
use crate::DatabaseStore;
use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement::funding::account::lock_funding_account;
use crate::database::settlement::funding::ledger::{complete, replay};
use crate::database::settlement::funding::{lock_funding_worlds, storage};

const ACTION: &str = "register_opening_attribution";

/// 份额的持久依据文本：固定值，不含任何外部内容。
const SLICE_BASIS: &str = "opening_attribution_v1";

/// 持久化的一段份额。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SliceRow {
    pub(super) id: Uuid,
    pub(super) entry_id: Uuid,
    pub(super) allocation_id: Uuid,
    pub(super) start: i64,
    pub(super) end: i64,
}

impl SliceRow {
    pub(super) const fn amount(&self) -> i64 {
        self.end - self.start
    }
}

/// 领域规则违反到资金错误：重叠是冲突，opening 阻断是不可用，其余是无效输入。
pub(super) fn rule(error: FundingRuleError) -> FundingError {
    match error {
        FundingRuleError::SliceOverlap => FundingError::Conflict,
        FundingRuleError::OpeningBlocked => FundingError::Unavailable,
        _ => FundingError::Invalid,
    }
}

/// 份额 ID：由实收业务键与区间确定，同一份额重复登记得到同一行。
fn slice_id(key_id: Uuid, slice: &OpeningSlice) -> Uuid {
    let mut bytes = b"auction.consignment.opening-slice.v1\0".to_vec();
    bytes.extend_from_slice(key_id.as_bytes());
    bytes.extend_from_slice(slice.entry_id.as_bytes());
    bytes.extend_from_slice(&slice.start.to_be_bytes());
    bytes.extend_from_slice(&slice.end.to_be_bytes());
    Uuid::new_v5(&Uuid::NAMESPACE_URL, &bytes)
}

/// 登记入口：封存并锁定逐笔依据，不发行、不转移。
///
/// 不追加订单阶段事件：命令只以实收业务键（UID + 付款 MID + 腿）为键、不指向任何合同，登记也不改变
/// 任何订单的阶段（rulings“资金写口的事件”：份额登记只写回执与审计）。份额被消费时的阶段变化由
/// `certify_cod` 在同事务追加。
pub(crate) async fn register_opening_attribution(
    store: &DatabaseStore,
    command: RegisterOpeningAttribution,
) -> Result<AttributionReceipt, FundingError> {
    if command.mutation.command_id.is_nil() || command.mutation.request_digest == [0; 32] {
        return Err(FundingError::Invalid);
    }
    // 先在应用边界拒绝空份额与区间重叠，不进事务。
    opening_included_total(&command.slices).map_err(rule)?;
    if command.slices.is_empty() {
        return Err(FundingError::Invalid);
    }
    run_serializable(store, &command, |tx, command| Box::pin(register_in_tx(tx, command))).await
}

async fn register_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    command: &RegisterOpeningAttribution,
) -> Result<AttributionReceipt, FundingError> {
    lock_funding_worlds(tx).await?;
    let account =
        lock_funding_account(tx, &command.receipt.uid).await?.ok_or(FundingError::Unavailable)?;
    let key_id = receipt_key_id(&command.receipt);
    let included = opening_included_total(&command.slices).map_err(rule)?;
    if replay(tx, &account, &command.mutation, ACTION, key_id).await? {
        return replayed_receipt(tx, command, key_id, included).await;
    }
    // opening 最终写门：本 UID 有未核清的现金退件阻断时拒绝，已通过的旧预览不能绕过。
    let blocked: bool = sqlx::query_scalar(
        r#"SELECT EXISTS (SELECT 1 FROM public.consignment_cash_return_blocks
                          WHERE uid = $1 AND state = 'open')"#,
    )
    .bind(&command.receipt.uid)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    if blocked {
        return Err(FundingError::Unavailable);
    }
    let existing = load_slices(tx, &command.receipt.uid, key_id).await?;
    if !existing.is_empty() {
        // 同一实收业务键已有份额：集合相同是另一命令的重放，不同即换份额，冲突。
        let receipt = replayed_receipt(tx, command, key_id, included).await?;
        record_replayed_command(tx, &command.receipt.uid, &command.mutation, ACTION, key_id)
            .await?;
        append_receipt(tx, command, key_id, included).await?;
        return Ok(receipt);
    }
    for slice in &command.slices {
        // 份额必须来自本 UID 的 opening 发行分录；边界取该 opening 来源的建立时刻。
        let inserted = sqlx::query(
            r#"INSERT INTO public.consignment_opening_slices
                   (id, uid, receipt_key_id, boundary_at, opening_source_id,
                    opening_issue_entry_id, original_allocation_id, amount_from, amount_to, basis)
               SELECT $1, entry.uid, $2, source.created_at, entry.source_id, entry.id,
                      entry.credit_id, $3, $4, $5
               FROM public.settlement_funding_entries AS entry
               JOIN public.settlement_funding_sources AS source ON source.id = entry.source_id
               WHERE entry.id = $6 AND entry.uid = $7 AND entry.kind = 'issue'
                 AND entry.credit_id IS NOT NULL
                 AND source.kind IN ('reconciliation', 'record_opening')"#,
        )
        .bind(slice_id(key_id, slice))
        .bind(key_id)
        .bind(slice.start)
        .bind(slice.end)
        .bind(SLICE_BASIS)
        .bind(slice.entry_id)
        .bind(&command.receipt.uid)
        .execute(&mut **tx)
        .await
        .map_err(funding_sql_error)?;
        if inserted.rows_affected() != 1 {
            return Err(FundingError::Unproven);
        }
    }
    complete(tx, &account, &command.mutation, ACTION, key_id).await?;
    append_receipt(tx, command, key_id, included).await?;
    Ok(AttributionReceipt { attribution_id: key_id, included, replayed: false })
}

/// 同事务追加 `attribution_registered` 回执与审计（不变量 1、3、8）；回执只带实收业务键、UID 与份额合计。
async fn append_receipt(
    tx: &mut Transaction<'_, Postgres>,
    command: &RegisterOpeningAttribution,
    key_id: Uuid,
    included: i64,
) -> Result<(), FundingError> {
    append_success(
        tx,
        SuccessReceipt {
            mutation: &command.mutation,
            operation: ACTION,
            target_kind: "receipt_key",
            target_id: key_id,
            disposition: "attribution_registered",
            fields: json!({
                "receipt_key_id": key_id.to_string(),
                "uid": command.receipt.uid,
                "attributed_amount": included,
            }),
        },
    )
    .await
}

/// 读取该实收业务键已封存的份额并与命令比对；集合不同是冲突。
async fn replayed_receipt(
    tx: &mut Transaction<'_, Postgres>,
    command: &RegisterOpeningAttribution,
    key_id: Uuid,
    included: i64,
) -> Result<AttributionReceipt, FundingError> {
    let stored = load_slices(tx, &command.receipt.uid, key_id).await?;
    let mut wanted: Vec<(Uuid, i64, i64)> =
        command.slices.iter().map(|slice| (slice.entry_id, slice.start, slice.end)).collect();
    wanted.sort_unstable();
    let mut have: Vec<(Uuid, i64, i64)> =
        stored.iter().map(|slice| (slice.entry_id, slice.start, slice.end)).collect();
    have.sort_unstable();
    if wanted != have {
        return Err(FundingError::Conflict);
    }
    Ok(AttributionReceipt { attribution_id: key_id, included, replayed: true })
}

/// 读取某实收业务键的全部份额。
pub(super) async fn load_slices(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    key_id: Uuid,
) -> Result<Vec<SliceRow>, FundingError> {
    let rows = sqlx::query(
        r#"SELECT id, opening_issue_entry_id, original_allocation_id, amount_from, amount_to
           FROM public.consignment_opening_slices
           WHERE receipt_key_id = $1 AND uid = $2
           ORDER BY opening_issue_entry_id, amount_from"#,
    )
    .bind(key_id)
    .bind(uid)
    .fetch_all(&mut **tx)
    .await
    .map_err(storage)?;
    rows.iter()
        .map(|row| {
            Ok(SliceRow {
                id: row.try_get("id").map_err(storage)?,
                entry_id: row.try_get("opening_issue_entry_id").map_err(storage)?,
                allocation_id: row.try_get("original_allocation_id").map_err(storage)?,
                start: row.try_get("amount_from").map_err(storage)?,
                end: row.try_get("amount_to").map_err(storage)?,
            })
        })
        .collect()
}

/// 消费前校验并汇总：份额未被任何实收消费、原 allocation 仍是未被他人占用的 opening 余额且足额。
///
/// 返回按 allocation 汇总的借出额。拍卖补偿池及其他受益人已占余额不能借用；原余额已花（不足额或
/// 状态非 available）与证不清分别以不可用与未证明失败关闭（不变量 6）。
pub(super) async fn borrow_plan(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    slices: &[SliceRow],
) -> Result<Vec<(Uuid, i64)>, FundingError> {
    let ids: Vec<Uuid> = slices.iter().map(|slice| slice.id).collect();
    let used: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.consignment_opening_slice_uses WHERE slice_id = ANY($1))",
    )
    .bind(&ids)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    if used {
        return Err(FundingError::Conflict);
    }
    let mut need: BTreeMap<Uuid, i64> = BTreeMap::new();
    for slice in slices {
        let total = need.entry(slice.allocation_id).or_insert(0);
        *total = total.checked_add(slice.amount()).ok_or(FundingError::Invalid)?;
    }
    for (allocation_id, amount) in &need {
        let row = sqlx::query(
            r#"SELECT purpose, amount, state,
                      (lot_id IS NULL AND consignment_contract_id IS NULL
                       AND obligation_id IS NULL AND delivery_id IS NULL) AS unowned
               FROM public.settlement_funding_allocations
               WHERE id = $1 AND uid = $2
               FOR UPDATE"#,
        )
        .bind(allocation_id)
        .bind(uid)
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage)?
        .ok_or(FundingError::Unproven)?;
        let purpose: String = row.try_get("purpose").map_err(storage)?;
        let unowned: bool = row.try_get("unowned").map_err(storage)?;
        if !unowned || !matches!(purpose.as_str(), "quarantine" | "proprietary") {
            return Err(FundingError::Unproven);
        }
        let state: String = row.try_get("state").map_err(storage)?;
        let balance: i64 = row.try_get("amount").map_err(storage)?;
        if state != "available" || balance < *amount {
            return Err(FundingError::Unavailable);
        }
    }
    Ok(need.into_iter().collect())
}

/// 把份额标记为被本笔实收消费；同一份额只能消费一次（主键冲突即被另一实收占用）。
pub(super) async fn insert_uses(
    tx: &mut Transaction<'_, Postgres>,
    slices: &[SliceRow],
    receipt_source_id: Uuid,
    key_id: Uuid,
) -> Result<(), FundingError> {
    for slice in slices {
        sqlx::query(
            r#"INSERT INTO public.consignment_opening_slice_uses
                   (slice_id, receipt_source_id, receipt_key_id)
               VALUES ($1, $2, $3)"#,
        )
        .bind(slice.id)
        .bind(receipt_source_id)
        .bind(key_id)
        .execute(&mut **tx)
        .await
        .map_err(funding_sql_error)?;
    }
    Ok(())
}
