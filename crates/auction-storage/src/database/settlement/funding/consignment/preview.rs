//! 资金预览的寄售段：本 UID 的现金退件阻断与全局 `record_only` 门（D9 §4.2、§10）。
//!
//! 保护不变量 2、6：预览只读，与 opening 最终写入口（`blockers::ensure_opening_allowed` 与
//! `reconciliation::apply` 的全局 `record_only` 门）使用同一事实，预览列出的拒绝原因就是最终写会
//! 拒绝的原因。A 的寄售退件阻断只归 A：本 UID 之外的阻断不进入本预览（异 UID 仍由原有的
//! `other_uid_protected_account_requires_reconciliation` 沿原对账门处理）；`record_only` 账户是
//! 全局事实，存在即对任一 UID 的现金 opening 标拒绝。

use auction_application::settlement::FundingError;
use serde_json::{Value, json};
use sqlx::{Postgres, Row};
use uuid::Uuid;

use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement::funding::storage;

/// 全局存在 `record_only` 账户时的拒绝码：现金开户写入口对任一 UID 一律拒绝。
const RECORD_ONLY_BLOCKER: &str = "record_only_account_blocks_cash_opening";

/// 预览的寄售段：拒绝原因与并入预览摘要的事实。
pub(crate) struct ConsignmentPreview {
    /// 并入预览阻断列表的拒绝原因，不含秘密（只含阻断所属 intake 与腿的稳定标识）。
    pub(crate) blockers: Vec<String>,
    /// 并入 `facts` 的寄售事实：本 UID 的未核清阻断与全局 `record_only` 门。
    pub(crate) facts: Value,
}

/// 读取本 UID 的未核清现金退件阻断与全局 `record_only` 状态。
pub(crate) async fn consignment_preview(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
) -> Result<ConsignmentPreview, FundingError> {
    let rows = sqlx::query(
        r#"SELECT intake_id, leg, original_delivery_id, amount, original_mode
           FROM public.consignment_cash_return_blocks
           WHERE uid=$1 AND state='open'
           ORDER BY intake_id, leg"#,
    )
    .bind(uid)
    .fetch_all(&mut **tx)
    .await
    .map_err(storage)?;
    let mut blockers = Vec::with_capacity(rows.len() + 1);
    let mut blocks = Vec::with_capacity(rows.len());
    for row in rows {
        let intake_id: Uuid = row.try_get("intake_id").map_err(storage)?;
        let leg: String = row.try_get("leg").map_err(storage)?;
        blockers.push(format!("consignment_cash_return:{intake_id}:{leg}:unconsumed"));
        blocks.push(json!({
            "intake_id": intake_id,
            "leg": leg,
            "original_delivery_id": row.try_get::<Uuid, _>("original_delivery_id").map_err(storage)?,
            "amount": row.try_get::<i64, _>("amount").map_err(storage)?,
            "original_mode": row.try_get::<String, _>("original_mode").map_err(storage)?,
        }));
    }
    let record_only_exists: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(
               SELECT 1 FROM public.settlement_funding_accounts WHERE accounting_mode='record_only'
           )"#,
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    if record_only_exists {
        blockers.push(RECORD_ONLY_BLOCKER.to_owned());
    }
    Ok(ConsignmentPreview {
        blockers,
        facts: json!({
            "open_cash_return_blocks": blocks,
            "record_only_account_exists": record_only_exists,
        }),
    })
}
