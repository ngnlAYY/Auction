//! 处理动作的准入事实：在调用方已按锁序锁定（或只读）的行上投影领域准入
//! [`SettlementReturnAdmission`]，命令事务、Worker 收口与只读详情共用同一口径。

use auction_application::settlement::SettlementReturnMatch;
use auction_domain::{
    SettlementObligationKind, SettlementReturnAdmission, SettlementReturnBinding,
    SettlementReturnFunding, SettlementReturnObligationPosition, cash_return,
};
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::cases::CaseRow;
use super::errors::ReturnTransactionError;
use crate::database::TrackedTransaction as Transaction;
use crate::database::repository_error;

/// 代管结案写入履约的稳定原因码。
pub(super) const HELD_FAILURE_CODE: &str = "settlement_return_held";

/// 留箱结案写入履约的稳定原因码。
pub(super) const INBOX_FAILURE_CODE: &str = "settlement_return_inbox";

/// 原义务的当前履约行。
#[derive(Debug, Clone)]
pub(super) struct FulfillmentRow {
    pub(super) state_version: i64,
    pub(super) execution_state: String,
    pub(super) delivery_id: Option<Uuid>,
    pub(super) dispatch_no: i32,
    pub(super) last_error_code: Option<String>,
}

/// 准入投影及其来源事实。
#[derive(Debug, Clone)]
pub(super) struct AdmissionContext {
    pub(super) admission: SettlementReturnAdmission,
    pub(super) fulfillment: Option<FulfillmentRow>,
}

/// 读取义务种类；义务缺失或种类未知时为空。
pub(super) async fn obligation_kind(
    tx: &mut Transaction<'_, Postgres>,
    obligation_id: Option<Uuid>,
) -> Result<Option<SettlementObligationKind>, ReturnTransactionError> {
    let Some(obligation_id) = obligation_id else {
        return Ok(None);
    };
    let kind: Option<String> = sqlx::query_scalar(
        "SELECT obligation_kind FROM public.settlement_obligations WHERE id = $1",
    )
    .bind(obligation_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(kind.as_deref().and_then(SettlementObligationKind::from_name))
}

/// 计算准入。`lock_fulfillment` 为真时锁定履约行（锁序在案件、动作与投递之后）；恢复谓词
/// `executable` 由调用方在锁定恢复记录与义务后给出，未绑定案件为空。
pub(super) async fn admission_facts(
    tx: &mut Transaction<'_, Postgres>,
    case: &CaseRow,
    executable: Option<bool>,
    lock_fulfillment: bool,
) -> Result<AdmissionContext, ReturnTransactionError> {
    let binding = if case.match_state == SettlementReturnMatch::Untrusted {
        SettlementReturnBinding::Untrusted
    } else if case.delivery_id.is_none() {
        SettlementReturnBinding::Unbound
    } else {
        SettlementReturnBinding::Bound
    };
    let obligation_kind = obligation_kind(tx, case.obligation_id).await?;
    let fulfillment = match case.obligation_id {
        Some(obligation_id) => load_fulfillment(tx, obligation_id, lock_fulfillment).await?,
        None => None,
    };
    let position = position(case, fulfillment.as_ref(), executable);
    let funding = match (obligation_kind, case.delivery_id) {
        (Some(kind), Some(delivery_id)) if cash_return(kind) => {
            cash_funding(tx, delivery_id, case.return_mail_id).await?
        }
        _ => SettlementReturnFunding::NotCash,
    };
    let take_proven: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM public.settlement_return_takes WHERE return_mail_id = $1)",
    )
    .bind(case.return_mail_id)
    .fetch_one(&mut **tx)
    .await?;
    let resends_used = fulfillment
        .as_ref()
        .map_or(0, |fulfillment| u32::try_from(fulfillment.dispatch_no - 1).unwrap_or(0));
    let resend_limit = u8::try_from(case.resend_limit)
        .ok()
        .ok_or_else(|| repository_error("结算退件补寄上限无效"))?;
    Ok(AdmissionContext {
        admission: SettlementReturnAdmission {
            status: case.status,
            binding,
            assets_match: case.reason_code == "returned_assets_match",
            obligation_kind,
            position,
            funding,
            resends_used,
            resend_limit,
            take_proven,
        },
        fulfillment,
    })
}

/// 现金退件原派发的资金关联：已支出（spent）或本案已返还为 `Funded`；占用过分配（当前指针或
/// 只追加的历史绑定）却既未支出也未返还为 `Unresolved`——原发送结果未知，必须先由 Sent 证明收口，
/// 不能当作“历史没有分配”直接代管（否则留下的 unknown 占用再也无法收口，strict_cash 下还会持续
/// 阻断同账户发送）；从未占用分配才是 `Unfunded`。
async fn cash_funding(
    tx: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
    return_mail_id: Uuid,
) -> Result<SettlementReturnFunding, ReturnTransactionError> {
    let (funded, allocated): (bool, bool) = sqlx::query_as(
        r#"SELECT EXISTS(
                      SELECT 1 FROM public.settlement_funding_allocations
                      WHERE delivery_id = $1 AND state = 'spent'
                        AND purpose IN ('seller', 'refund')
                  ) OR EXISTS(
                      SELECT 1 FROM public.settlement_funding_returns
                      WHERE delivery_id = $1 AND return_mail_id = $2
                  ),
                  EXISTS(
                      SELECT 1 FROM public.settlement_funding_allocations WHERE delivery_id = $1
                  ) OR EXISTS(
                      SELECT 1 FROM public.settlement_funding_delivery_bindings
                      WHERE delivery_id = $1
                  )"#,
    )
    .bind(delivery_id)
    .bind(return_mail_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(match (funded, allocated) {
        (true, _) => SettlementReturnFunding::Funded,
        (false, true) => SettlementReturnFunding::Unresolved,
        (false, false) => SettlementReturnFunding::Unfunded,
    })
}

async fn load_fulfillment(
    tx: &mut Transaction<'_, Postgres>,
    obligation_id: Uuid,
    lock: bool,
) -> Result<Option<FulfillmentRow>, ReturnTransactionError> {
    let query = if lock {
        r#"SELECT state_version, execution_state, delivery_id, dispatch_no, last_error_code
           FROM public.settlement_obligation_fulfillments
           WHERE obligation_id = $1 FOR UPDATE"#
    } else {
        r#"SELECT state_version, execution_state, delivery_id, dispatch_no, last_error_code
           FROM public.settlement_obligation_fulfillments
           WHERE obligation_id = $1"#
    };
    let row = sqlx::query(query).bind(obligation_id).fetch_optional(&mut **tx).await?;
    row.map(|row| {
        Ok(FulfillmentRow {
            state_version: row.try_get("state_version")?,
            execution_state: row.try_get("execution_state")?,
            delivery_id: row.try_get("delivery_id")?,
            dispatch_no: row.try_get("dispatch_no")?,
            last_error_code: row.try_get("last_error_code")?,
        })
    })
    .transpose()
}

/// 原义务与退件派发的关系：已满足优先；派发不再是当前派发即被取代；成交撤销或履约确定失败为
/// 业务终结；履约因本案转人工对账、或因本案代管/留箱而终结时才是当前。
fn position(
    case: &CaseRow,
    fulfillment: Option<&FulfillmentRow>,
    executable: Option<bool>,
) -> SettlementReturnObligationPosition {
    use SettlementReturnObligationPosition as P;

    let Some(fulfillment) = fulfillment else {
        return P::Superseded;
    };
    if fulfillment.execution_state == "satisfied" {
        return P::Satisfied;
    }
    if case.delivery_id.is_none() || fulfillment.delivery_id != case.delivery_id {
        return P::Superseded;
    }
    if executable != Some(true) {
        return P::Ended;
    }
    match fulfillment.execution_state.as_str() {
        "manual_reconcile" => P::Current,
        "failed"
            if matches!(
                fulfillment.last_error_code.as_deref(),
                Some(HELD_FAILURE_CODE | INBOX_FAILURE_CODE)
            ) =>
        {
            P::Current
        }
        "failed" => P::Ended,
        _ => P::Superseded,
    }
}
