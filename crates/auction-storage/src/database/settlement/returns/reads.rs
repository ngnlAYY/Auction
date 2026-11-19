//! Admin 结算退件只读投影：列表、详情与待办计数。以案件/证据为主表，LEFT JOIN 义务与投递，
//! 未知义务与隔离记录不会被联表丢失；详情的允许动作与阻断原因由命令事务同一准入规则计算，
//! 只读快照不授予任何动作，命令事务仍会在锁内复核。

use auction_application::core::ApplicationError;
use auction_application::settlement::{
    SettlementReturnAssetLocation, SettlementReturnCaseSummary, SettlementReturnCustody,
    SettlementReturnDetail, SettlementReturnDispatch, SettlementReturnError, SettlementReturnEvent,
    SettlementReturnListRequest, SettlementReturnMatch, SettlementReturnPage,
    SettlementReturnReason, SettlementReturnTake, settlement_return_assets,
};
use auction_domain::{
    CommandId, SettlementObligationKind, SettlementReturnAction, SettlementReturnCaseStatus,
    SettlementReturnPlan,
};
use sqlx::postgres::PgRow;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::admission::admission_facts;
use super::cases::{current_return_evidence_sql, load_case};
use super::errors::ReturnTransactionError;
use super::observed::take_unknown_reason;
use crate::database::TrackedTransaction as Transaction;
use crate::database::repository_error;
use crate::database::settlement::fulfillment::recovery_allows_execution_now;

/// 列表与详情共用的摘要投影（别名见 FROM）；资产取案件当前有效证据（核验改绑后为核验事实）。
macro_rules! summary_select {
    () => {
        concat!(
            r#"SELECT c.return_mail_id, c.case_version, c.status, e.match_state, c.reason_code,
                  c.world_id, i.external_mail_id, i.received_at, c.obligation_id,
                  o.obligation_kind, o.auction_id, o.lot_id, c.delivery_id, c.dispatch_no,
                  d.recipient, "#,
            current_return_evidence_sql!("c", "i"),
            r#" AS decision_metadata, c.plan, c.resend_limit,
                  f.dispatch_no AS current_dispatch_no, c.created_at, c.updated_at, c.closed_at
           FROM public.settlement_return_cases AS c
           JOIN public.settlement_return_evidence AS e ON e.return_mail_id = c.return_mail_id
           JOIN public.inbound_mail AS i ON i.id = c.return_mail_id
           LEFT JOIN public.settlement_obligations AS o ON o.id = c.obligation_id
           LEFT JOIN public.deliveries AS d ON d.id = c.delivery_id
           LEFT JOIN public.settlement_obligation_fulfillments AS f
             ON f.obligation_id = c.obligation_id"#
        )
    };
}

/// 列表筛选条件（参数 $1..$8）。
macro_rules! list_filter {
    () => {
        r#" WHERE ($1::text IS NULL OR c.world_id = $1)
              AND ($2::text IS NULL OR e.match_state = $2)
              AND ($3::text IS NULL OR c.status = $3)
              AND ($4::text IS NULL OR o.obligation_kind = $4)
              AND (NOT $5 OR c.closed_at IS NULL)
              AND ($6::timestamptz IS NULL OR i.received_at >= $6)
              AND ($7::timestamptz IS NULL OR i.received_at <= $7)
              AND ($8::uuid IS NULL OR c.return_mail_id = $8 OR c.obligation_id = $8
                   OR c.delivery_id = $8)"#
    };
}

/// 列表总数（参数 $1..$8 见 `list_filter!`）。
pub(super) const LIST_COUNT_SQL: &str = concat!(
    r#"SELECT count(*)
           FROM public.settlement_return_cases AS c
           JOIN public.settlement_return_evidence AS e ON e.return_mail_id = c.return_mail_id
           JOIN public.inbound_mail AS i ON i.id = c.return_mail_id
           LEFT JOIN public.settlement_obligations AS o ON o.id = c.obligation_id"#,
    list_filter!()
);

/// 列表当前页：筛选同 `LIST_COUNT_SQL`，`$9` 页长、`$10` 偏移，按收件时间倒序。
pub(super) const LIST_PAGE_SQL: &str = concat!(
    summary_select!(),
    list_filter!(),
    " ORDER BY i.received_at DESC, c.return_mail_id DESC LIMIT $9 OFFSET $10"
);

async fn begin_read(
    pool: &crate::database::TrackedPool<Postgres>,
) -> Result<Transaction<'static, Postgres>, ReturnTransactionError> {
    Ok(pool.begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY").await?)
}

/// 分页列表；总数与当前页读自同一快照。
pub(super) async fn list_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    request: &SettlementReturnListRequest,
) -> Result<SettlementReturnPage, ApplicationError> {
    list_once(pool, request).await.map_err(ReturnTransactionError::into_application)
}

async fn list_once(
    pool: &crate::database::TrackedPool<Postgres>,
    request: &SettlementReturnListRequest,
) -> Result<SettlementReturnPage, ReturnTransactionError> {
    if !request.validate() {
        return Err(SettlementReturnError::Invalid.into());
    }
    let mut tx = begin_read(pool).await?;
    let match_state = request.match_state.map(SettlementReturnMatch::as_str);
    let status = request.status.map(SettlementReturnCaseStatus::as_str);
    let kind = request.obligation_kind.map(SettlementObligationKind::as_str);
    let total: i64 = sqlx::query_scalar(LIST_COUNT_SQL)
        .bind(request.world.as_deref())
        .bind(match_state)
        .bind(status)
        .bind(kind)
        .bind(request.open_only)
        .bind(request.received_from)
        .bind(request.received_to)
        .bind(request.reference)
        .fetch_one(&mut *tx)
        .await?;
    let rows = sqlx::query(LIST_PAGE_SQL)
        .bind(request.world.as_deref())
        .bind(match_state)
        .bind(status)
        .bind(kind)
        .bind(request.open_only)
        .bind(request.received_from)
        .bind(request.received_to)
        .bind(request.reference)
        .bind(request.limit)
        .bind(request.offset)
        .fetch_all(&mut *tx)
        .await?;
    let items = rows.iter().map(map_summary).collect::<Result<Vec<_>, _>>()?;
    tx.commit().await?;
    Ok(SettlementReturnPage { items, total })
}

/// 单个案件详情；不存在时返回 [`SettlementReturnError::NotFound`]。
pub(super) async fn detail_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    return_mail_id: Uuid,
) -> Result<SettlementReturnDetail, ApplicationError> {
    detail_once(pool, return_mail_id).await.map_err(ReturnTransactionError::into_application)
}

async fn detail_once(
    pool: &crate::database::TrackedPool<Postgres>,
    return_mail_id: Uuid,
) -> Result<SettlementReturnDetail, ReturnTransactionError> {
    if return_mail_id.is_nil() {
        return Err(SettlementReturnError::Invalid.into());
    }
    let mut tx = begin_read(pool).await?;
    let row = sqlx::query(concat!(summary_select!(), " WHERE c.return_mail_id = $1"))
        .bind(return_mail_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(SettlementReturnError::NotFound)?;
    let summary = map_summary(&row)?;
    let case =
        load_case(&mut tx, return_mail_id, false).await?.ok_or(SettlementReturnError::NotFound)?;
    let executable = match case.obligation_id {
        Some(obligation_id) if case.delivery_id.is_some() => {
            Some(recovery_allows_execution_now(&mut tx, obligation_id).await?)
        }
        _ => None,
    };
    let context = admission_facts(&mut tx, &case, executable, false).await?;
    let mut allowed_actions = Vec::new();
    let mut blockers = Vec::new();
    for action in SettlementReturnAction::ALL {
        match context.admission.check(*action) {
            Ok(()) => allowed_actions.push(*action),
            Err(blocker) => blockers.push((*action, blocker)),
        }
    }
    let events = read_events(&mut tx, return_mail_id).await?;
    let dispatches = match case.obligation_id {
        Some(obligation_id) => read_dispatches(&mut tx, obligation_id).await?,
        None => Vec::new(),
    };
    let take = read_take(&mut tx, return_mail_id).await?;
    let custody = if case.status == SettlementReturnCaseStatus::Held {
        read_custody(&mut tx, return_mail_id).await?
    } else {
        None
    };
    let recheck_unavailable = case.status == SettlementReturnCaseStatus::RecheckPending
        && !worker_capability(&mut tx, &case.world_id, RECHECK_CAPABILITY).await?;
    // 待领回的 Take 尚未执行（动作未激活，或已激活但投递仍待领取），而没有 Worker 能以持久凭据
    // 身份执行该世界的 Take：说明案件为何停在待领回。
    let take_waiting = take.as_ref().is_none_or(|take| {
        take.execution_state == "pending" || take.delivery_status.as_deref() == Some("pending")
    });
    let take_unavailable = case.status == SettlementReturnCaseStatus::TakePending
        && take_waiting
        && !worker_capability(&mut tx, &case.world_id, TAKE_CAPABILITY).await?;
    // 结果未知时说明具体原因：最近一次原会话观察，没有观察时由持久事实推出。
    let take_unknown_reason = if case.status == SettlementReturnCaseStatus::TakeUnknown {
        take_unknown_reason(&mut tx, return_mail_id).await?
    } else {
        None
    };
    tx.commit().await?;
    Ok(SettlementReturnDetail {
        summary,
        events,
        dispatches,
        take,
        resend_dispatch_no: case.resend_dispatch_no.and_then(|value| u32::try_from(value).ok()),
        custody,
        allowed_actions,
        blockers,
        recheck_unavailable,
        take_unavailable,
        take_unknown_reason,
    })
}

/// Worker 能力判定（参数 `$1` 为世界，`$2` 为心跳详情里声明能力的键：只读核验为
/// `settlement_return_recheck_worlds`，以持久凭据身份执行 Take 为 `settlement_return_take_worlds`）。
/// 满足任一条件即可用：
///
/// - 有效期内（`expires_at` 未到）的非停止心跳声明了该世界；
/// - 宽限窗口：最近一次 Worker 心跳（全部实例按观察时刻取最新）未处于停止中、声明了该世界，且
///   距过期不超过该心跳自身的一个 TTL（`expires_at - observed_at`，Worker 为 30 秒，每 10 秒
///   发布）。进程崩溃或被强制结束后重启、发布偶发失败时，旧心跳过期到新实例发布 `starting` 之间
///   不会短暂显示不可用。
///
/// 最近一次心跳处于停止中（正常停止或重启时旧实例最后发布 `stopping`）、未声明该世界或该键，或
/// 已超出宽限窗口时都不可用（失败关闭：显示能力不可用，不改变案件）。
const WORKER_CAPABILITY_SQL: &str = r#"WITH sql_clock AS MATERIALIZED (SELECT clock_timestamp() AS at),
    workers AS (
    SELECT instance_id, status, observed_at, expires_at,
           COALESCE(jsonb_typeof(details -> $2::text) = 'array'
                    AND details -> $2::text ? $1, false) AS declares
    FROM public.service_heartbeats
    WHERE service_name = 'auction-worker' AND process_role = 'worker'
      AND (SELECT at FROM sql_clock) < expires_at + (expires_at - observed_at)
)
SELECT EXISTS (
           SELECT 1 FROM workers
           WHERE declares AND status IN ('starting', 'ready', 'degraded')
             AND expires_at > (SELECT at FROM sql_clock)
       )
       OR COALESCE((
           SELECT declares AND status IN ('starting', 'ready', 'degraded')
           FROM workers
           ORDER BY observed_at DESC, instance_id DESC
           LIMIT 1
       ), false)"#;

/// 心跳详情声明只读核验能力的键。
const RECHECK_CAPABILITY: &str = "settlement_return_recheck_worlds";
/// 心跳详情声明能以持久凭据身份执行结算退件 Take 的键。
const TAKE_CAPABILITY: &str = "settlement_return_take_worlds";

/// 是否有 Worker 声明了该世界的能力；口径见 [`WORKER_CAPABILITY_SQL`]。
async fn worker_capability(
    tx: &mut Transaction<'_, Postgres>,
    world_id: &str,
    capability: &str,
) -> Result<bool, ReturnTransactionError> {
    let available: bool = sqlx::query_scalar(WORKER_CAPABILITY_SQL)
        .bind(world_id)
        .bind(capability)
        .fetch_one(&mut **tx)
        .await?;
    Ok(available)
}

/// 仍需处理的案件数（导航待办）。
pub(super) async fn open_count_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
) -> Result<i64, ApplicationError> {
    sqlx::query_scalar(
        "SELECT count(*) FROM public.settlement_return_cases WHERE closed_at IS NULL",
    )
    .fetch_one(pool)
    .await
    .map_err(|error| ReturnTransactionError::from(error).into_application())
}

fn map_summary(row: &PgRow) -> Result<SettlementReturnCaseSummary, ReturnTransactionError> {
    let status: String = row.try_get("status")?;
    let status = SettlementReturnCaseStatus::from_name(&status)
        .ok_or_else(|| repository_error("结算退件案件状态无效"))?;
    let match_state: String = row.try_get("match_state")?;
    let reason: String = row.try_get("reason_code")?;
    let kind: Option<String> = row.try_get("obligation_kind")?;
    let plan: Option<String> = row.try_get("plan")?;
    let delivery_id: Option<Uuid> = row.try_get("delivery_id")?;
    let metadata: serde_json::Value = row.try_get("decision_metadata")?;
    let assets = settlement_return_assets(&metadata);
    let current_dispatch: Option<i32> = row.try_get("current_dispatch_no")?;
    let resend_limit: i16 = row.try_get("resend_limit")?;
    Ok(SettlementReturnCaseSummary {
        return_mail_id: row.try_get("return_mail_id")?,
        case_version: row.try_get("case_version")?,
        status,
        match_state: SettlementReturnMatch::from_name(&match_state)
            .ok_or_else(|| repository_error("结算退件证据分类无效"))?,
        reason: SettlementReturnReason::from_name(&reason)
            .ok_or_else(|| repository_error("结算退件原因码无效"))?,
        world_id: row.try_get("world_id")?,
        external_mail_id: row.try_get("external_mail_id")?,
        received_at: row.try_get("received_at")?,
        obligation_id: row.try_get("obligation_id")?,
        obligation_kind: kind.as_deref().and_then(SettlementObligationKind::from_name),
        auction_id: row.try_get("auction_id")?,
        lot_id: row.try_get("lot_id")?,
        delivery_id,
        dispatch_no: row
            .try_get::<Option<i32>, _>("dispatch_no")?
            .and_then(|value| u32::try_from(value).ok()),
        recipient: row.try_get("recipient")?,
        credits: assets.as_ref().map_or(0, |assets| assets.credits),
        cod: assets.as_ref().map_or(0, |assets| assets.cod),
        attachment_count: assets
            .as_ref()
            .map_or(0, |assets| u32::try_from(assets.attachment_ids.len()).unwrap_or(u32::MAX)),
        plan: plan.as_deref().and_then(SettlementReturnPlan::from_name),
        resend_limit: u8::try_from(resend_limit).unwrap_or(0),
        resends_used: current_dispatch.and_then(|value| u32::try_from(value - 1).ok()),
        asset_location: SettlementReturnAssetLocation::of(status, delivery_id.is_some()),
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        closed_at: row.try_get("closed_at")?,
    })
}

async fn read_events(
    tx: &mut Transaction<'_, Postgres>,
    return_mail_id: Uuid,
) -> Result<Vec<SettlementReturnEvent>, ReturnTransactionError> {
    let rows = sqlx::query(
        r#"SELECT case_version, event_kind, occurred_at, actor_id, command_id, details
           FROM public.settlement_return_events
           WHERE return_mail_id = $1
           ORDER BY case_version"#,
    )
    .bind(return_mail_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(SettlementReturnEvent {
                case_version: row.try_get("case_version")?,
                event_kind: row.try_get("event_kind")?,
                occurred_at: row.try_get("occurred_at")?,
                actor_id: row.try_get("actor_id")?,
                command_id: row.try_get::<Option<Uuid>, _>("command_id")?.map(CommandId::from_uuid),
                details: row.try_get("details")?,
            })
        })
        .collect()
}

async fn read_dispatches(
    tx: &mut Transaction<'_, Postgres>,
    obligation_id: Uuid,
) -> Result<Vec<SettlementReturnDispatch>, ReturnTransactionError> {
    let rows = sqlx::query(
        r#"SELECT dispatch.dispatch_no, dispatch.delivery_id, dispatch.authorized_by,
                  delivery.status, delivery.sending_at, delivery.completed_at
           FROM public.settlement_fulfillment_dispatches AS dispatch
           LEFT JOIN public.deliveries AS delivery ON delivery.id = dispatch.delivery_id
           WHERE dispatch.obligation_id = $1
           ORDER BY dispatch.dispatch_no"#,
    )
    .bind(obligation_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(SettlementReturnDispatch {
                dispatch_no: u32::try_from(row.try_get::<i32, _>("dispatch_no")?)
                    .ok()
                    .ok_or_else(|| repository_error("结算派发编号无效"))?,
                delivery_id: row.try_get("delivery_id")?,
                delivery_status: row.try_get("status")?,
                sending_at: row.try_get("sending_at")?,
                completed_at: row.try_get("completed_at")?,
                authorized_by: row.try_get("authorized_by")?,
            })
        })
        .collect()
}

async fn read_take(
    tx: &mut Transaction<'_, Postgres>,
    return_mail_id: Uuid,
) -> Result<Option<SettlementReturnTake>, ReturnTransactionError> {
    let row = sqlx::query(
        r#"SELECT action.execution_state, action.delivery_id, action.failure_code,
                  delivery.status AS delivery_status, take.proven_at
           FROM public.settlement_return_actions AS action
           LEFT JOIN public.deliveries AS delivery ON delivery.id = action.delivery_id
           LEFT JOIN public.settlement_return_takes AS take
             ON take.return_mail_id = action.return_mail_id
           WHERE action.return_mail_id = $1"#,
    )
    .bind(return_mail_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| {
        Ok(SettlementReturnTake {
            execution_state: row.try_get("execution_state")?,
            delivery_id: row.try_get("delivery_id")?,
            delivery_status: row.try_get("delivery_status")?,
            failure_code: row.try_get("failure_code")?,
            proven_at: row.try_get("proven_at")?,
        })
    })
    .transpose()
}

async fn read_custody(
    tx: &mut Transaction<'_, Postgres>,
    return_mail_id: Uuid,
) -> Result<Option<SettlementReturnCustody>, ReturnTransactionError> {
    let row = sqlx::query(
        r#"SELECT delivery.recipient, take.credits, take.attachment_ids
           FROM public.settlement_return_takes AS take
           JOIN public.deliveries AS delivery ON delivery.id = take.original_delivery_id
           WHERE take.return_mail_id = $1"#,
    )
    .bind(return_mail_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| {
        Ok(SettlementReturnCustody {
            beneficiary: row.try_get("recipient")?,
            credits: row.try_get("credits")?,
            equipment_ids: row.try_get("attachment_ids")?,
        })
    })
    .transpose()
}
