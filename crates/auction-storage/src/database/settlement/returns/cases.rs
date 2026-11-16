//! 退件案件投影与只追加事件的写入原语；调用方持有事务并已按锁序取得前缀锁。案件每次推进都
//! 以 `case_version` 比较交换，并在同一事务追加恰好一条事件（事件主键即推进后的版本）。

use auction_application::settlement::{
    SettlementReturnDecision, SettlementReturnError, SettlementReturnMatch,
};
use auction_domain::{
    CommandId, SETTLEMENT_RETURN_RESEND_LIMIT, SettlementReturnCaseStatus, SettlementReturnPlan,
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::errors::ReturnTransactionError;
use crate::database::TrackedTransaction as Transaction;
use crate::database::repository_error;

/// 已读取的案件投影及其不可变证据。
#[derive(Debug, Clone)]
pub(super) struct CaseRow {
    pub(super) return_mail_id: Uuid,
    pub(super) world_id: String,
    pub(super) obligation_id: Option<Uuid>,
    /// 已绑定的原派发投递。
    pub(super) delivery_id: Option<Uuid>,
    pub(super) dispatch_no: Option<i32>,
    pub(super) status: SettlementReturnCaseStatus,
    pub(super) reason_code: String,
    pub(super) plan: Option<SettlementReturnPlan>,
    pub(super) resend_limit: i16,
    pub(super) resend_dispatch_no: Option<i32>,
    pub(super) case_version: i64,
    /// 证据的原分类；`untrusted` 永远不能经核验改绑。
    pub(super) match_state: SettlementReturnMatch,
    /// 退件在收件箱中的 MID。
    pub(super) external_mail_id: String,
    /// 证据原文（`inbound_mail.decision_metadata`）。
    pub(super) metadata: Value,
}

/// 消费退件时建立案件：已绑定原派发的进入待决定，其余隔离；补寄上限按领域常量冻结进案件行。
pub(crate) async fn open_case(
    tx: &mut Transaction<'_, Postgres>,
    return_mail_id: Uuid,
    world_id: &str,
    obligation_id: Option<Uuid>,
    bound: Option<(Uuid, u32)>,
    decision: SettlementReturnDecision,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let status = if bound.is_some() {
        SettlementReturnCaseStatus::AwaitingDecision
    } else {
        SettlementReturnCaseStatus::Quarantined
    };
    let dispatch_no = bound.and_then(|(_, dispatch_no)| i32::try_from(dispatch_no).ok());
    sqlx::query(
        r#"INSERT INTO public.settlement_return_cases
               (return_mail_id, world_id, obligation_id, delivery_id, dispatch_no, status,
                reason_code, resend_limit, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $9)"#,
    )
    .bind(return_mail_id)
    .bind(world_id)
    .bind(obligation_id)
    .bind(bound.map(|(delivery_id, _)| delivery_id))
    .bind(dispatch_no)
    .bind(status.as_str())
    .bind(decision.reason.as_str())
    .bind(i16::from(SETTLEMENT_RETURN_RESEND_LIMIT))
    .bind(now)
    .execute(&mut **tx)
    .await?;
    append_event(
        tx,
        return_mail_id,
        1,
        status,
        CaseEvent {
            kind: "opened",
            command: None,
            details: json!({
                "match_state": decision.matched.as_str(),
                "reason_code": decision.reason.as_str(),
                "dispatch_no": dispatch_no,
            }),
        },
        now,
    )
    .await
}

/// 原派发投递是否已被某个退件案件绑定（任何状态）。Sent 证明据此判断履约是否已由案件接管；
/// 只读，依赖调用方的 Serializable 事务与案件命令互斥。
pub(crate) async fn return_case_bound(
    tx: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM public.settlement_return_cases WHERE delivery_id = $1)",
    )
    .bind(delivery_id)
    .fetch_one(&mut **tx)
    .await
}

/// 案件投影的 SELECT 片段（别名 `c`、`e`、`i`）。
macro_rules! case_select {
    () => {
        r#"SELECT c.return_mail_id, c.world_id, c.obligation_id, c.delivery_id, c.dispatch_no,
                  c.status, c.reason_code, c.plan, c.resend_limit, c.resend_dispatch_no,
                  c.case_version, e.match_state, i.external_mail_id,
                  i.decision_metadata
           FROM public.settlement_return_cases AS c
           JOIN public.settlement_return_evidence AS e ON e.return_mail_id = c.return_mail_id
           JOIN public.inbound_mail AS i ON i.id = c.return_mail_id"#
    };
}

/// 读取案件与其证据；`lock` 为真时锁定案件行（调用方须已取得锁序中更早的锁）。
pub(super) async fn load_case(
    tx: &mut Transaction<'_, Postgres>,
    return_mail_id: Uuid,
    lock: bool,
) -> Result<Option<CaseRow>, ReturnTransactionError> {
    let query = if lock {
        concat!(case_select!(), " WHERE c.return_mail_id = $1 FOR UPDATE OF c")
    } else {
        concat!(case_select!(), " WHERE c.return_mail_id = $1")
    };
    let row = sqlx::query(query).bind(return_mail_id).fetch_optional(&mut **tx).await?;
    row.as_ref().map(map_case).transpose()
}

fn map_case(row: &sqlx::postgres::PgRow) -> Result<CaseRow, ReturnTransactionError> {
    let status: String = row.try_get("status")?;
    let plan: Option<String> = row.try_get("plan")?;
    let match_state: String = row.try_get("match_state")?;
    Ok(CaseRow {
        return_mail_id: row.try_get("return_mail_id")?,
        world_id: row.try_get("world_id")?,
        obligation_id: row.try_get("obligation_id")?,
        delivery_id: row.try_get("delivery_id")?,
        dispatch_no: row.try_get("dispatch_no")?,
        status: SettlementReturnCaseStatus::from_name(&status)
            .ok_or_else(|| repository_error("结算退件案件状态无效"))?,
        reason_code: row.try_get("reason_code")?,
        plan: plan
            .map(|plan| {
                SettlementReturnPlan::from_name(&plan)
                    .ok_or_else(|| repository_error("结算退件处理计划无效"))
            })
            .transpose()?,
        resend_limit: row.try_get("resend_limit")?,
        resend_dispatch_no: row.try_get("resend_dispatch_no")?,
        case_version: row.try_get("case_version")?,
        match_state: SettlementReturnMatch::from_name(&match_state)
            .ok_or_else(|| repository_error("结算退件证据分类无效"))?,
        external_mail_id: row.try_get("external_mail_id")?,
        metadata: row.try_get("decision_metadata")?,
    })
}

/// 案件的下一状态；未改动的字段沿用当前值。
#[derive(Debug, Clone)]
pub(super) struct CaseTransition {
    pub(super) status: SettlementReturnCaseStatus,
    pub(super) plan: Option<SettlementReturnPlan>,
    pub(super) resend_dispatch_no: Option<i32>,
    pub(super) delivery_id: Option<Uuid>,
    pub(super) dispatch_no: Option<i32>,
    pub(super) reason_code: String,
    /// 完成一次只读重新核验时计数加一。
    pub(super) recheck_completed: bool,
}

impl CaseTransition {
    /// 以当前投影为起点构造下一状态。
    pub(super) fn from_case(case: &CaseRow, status: SettlementReturnCaseStatus) -> Self {
        Self {
            status,
            plan: case.plan,
            resend_dispatch_no: case.resend_dispatch_no,
            delivery_id: case.delivery_id,
            dispatch_no: case.dispatch_no,
            reason_code: case.reason_code.clone(),
            recheck_completed: false,
        }
    }
}

/// 以版本比较交换推进案件；版本已变化时返回 [`SettlementReturnError::Stale`]。返回推进后的版本。
pub(super) async fn advance_case(
    tx: &mut Transaction<'_, Postgres>,
    case: &CaseRow,
    next: &CaseTransition,
    now: DateTime<Utc>,
) -> Result<i64, ReturnTransactionError> {
    let closed = !next.status.open();
    let version: Option<i64> = sqlx::query_scalar(
        r#"UPDATE public.settlement_return_cases
           SET status = $3,
               plan = $4,
               resend_dispatch_no = $5,
               delivery_id = $6,
               dispatch_no = $7,
               reason_code = $8,
               recheck_count = recheck_count + CASE WHEN $9 THEN 1 ELSE 0 END,
               recheck_claim_token = NULL,
               recheck_claim_expires_at = NULL,
               closed_at = CASE WHEN $10 THEN $11 ELSE NULL END,
               case_version = case_version + 1,
               updated_at = $11
           WHERE return_mail_id = $1 AND case_version = $2
           RETURNING case_version"#,
    )
    .bind(case.return_mail_id)
    .bind(case.case_version)
    .bind(next.status.as_str())
    .bind(next.plan.map(SettlementReturnPlan::as_str))
    .bind(next.resend_dispatch_no)
    .bind(next.delivery_id)
    .bind(next.dispatch_no)
    .bind(&next.reason_code)
    .bind(next.recheck_completed)
    .bind(closed)
    .bind(now)
    .fetch_optional(&mut **tx)
    .await?;
    version.ok_or(ReturnTransactionError::Business(SettlementReturnError::Stale))
}

/// 一条处理事件：Admin 命令带命令 ID 与 actor，Worker 事件两者皆空。
pub(super) struct CaseEvent<'a> {
    pub(super) kind: &'static str,
    pub(super) command: Option<(CommandId, &'a str)>,
    pub(super) details: Value,
}

/// 只追加一条事件；主键 `(return_mail_id, case_version)` 保证每个版本恰好一条。
pub(super) async fn append_event(
    tx: &mut Transaction<'_, Postgres>,
    return_mail_id: Uuid,
    case_version: i64,
    status: SettlementReturnCaseStatus,
    event: CaseEvent<'_>,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"INSERT INTO public.settlement_return_events
               (return_mail_id, case_version, event_kind, status, command_id, actor_id, details,
                occurred_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"#,
    )
    .bind(return_mail_id)
    .bind(case_version)
    .bind(event.kind)
    .bind(status.as_str())
    .bind(event.command.map(|(command_id, _)| command_id.as_uuid()))
    .bind(event.command.map(|(_, actor_id)| actor_id))
    .bind(event.details)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// 推进案件并追加对应事件；返回推进后的版本。
pub(super) async fn transition(
    tx: &mut Transaction<'_, Postgres>,
    case: &CaseRow,
    next: &CaseTransition,
    event: CaseEvent<'_>,
    now: DateTime<Utc>,
) -> Result<i64, ReturnTransactionError> {
    let version = advance_case(tx, case, next, now).await?;
    append_event(tx, case.return_mail_id, version, next.status, event, now).await?;
    Ok(version)
}
