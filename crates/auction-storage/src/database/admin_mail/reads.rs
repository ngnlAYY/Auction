//! PostgreSQL Admin 邮件轮询状态、dashboard 与入站记录查询。

use super::mapping::{map_intake_summary, map_mail_poller, map_poll_state, storage_error};
use super::*;

pub(super) async fn get_poll_state_postgres(
    store: &crate::database::PostgresStore,
    request: AdminMailPollStateRequest,
) -> Result<AdminMailPollState, ApplicationError> {
    let row = sqlx::query(
        "SELECT mailbox_key, cursor_value, last_poll_at, next_poll_at,
                consecutive_failures, paused, state_version, updated_at
         FROM public.mail_poll_state WHERE mailbox_key = $1",
    )
    .bind(request.mailbox_key)
    .fetch_optional(store.pool())
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 邮件轮询状态失败"))?;
    let Some(row) = row else {
        return Err(ApplicationError::AdminMailTransaction(AdminMailTransactionError::NotFound));
    };
    let state = map_poll_state(row).map_err(ApplicationError::AdminMailTransaction)?;
    if state.validate() {
        Ok(state)
    } else {
        Err(ApplicationError::AdminMail(AdminMailError::InvalidProjection))
    }
}

pub(super) async fn dashboard_snapshot_postgres(
    store: &crate::database::PostgresStore,
) -> Result<AdminMailDashboardSnapshot, ApplicationError> {
    // 轮询器数量是部署配置的一小组固定状态；LIMIT + 1 让损坏或异常膨胀的表不会无限进入
    // Admin 响应，同时仍能明确报告超出安全投影上限的情况。
    let rows = sqlx::query(
        "SELECT mailbox_key, last_poll_at, next_poll_at, consecutive_failures,
                paused, state_version, updated_at
         FROM public.mail_poll_state
         ORDER BY mailbox_key ASC
         LIMIT 65",
    )
    .fetch_all(store.pool())
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 邮件轮询器 dashboard 摘要失败"))?;
    if rows.len() > MAX_ADMIN_MAIL_POLLER_COUNT {
        return Err(ApplicationError::AdminMail(AdminMailError::InvalidProjection));
    }
    let mut pollers = Vec::with_capacity(rows.len());
    for row in &rows {
        let poller = map_mail_poller(row)?;
        if !poller.validate()
            || pollers.iter().any(|previous: &AdminMailPollerSnapshot| {
                previous.mailbox_key == poller.mailbox_key
            })
        {
            return Err(ApplicationError::AdminMail(AdminMailError::InvalidProjection));
        }
        pollers.push(poller);
    }
    let snapshot = AdminMailDashboardSnapshot { observed_at: Utc::now(), pollers };
    if snapshot.validate() {
        Ok(snapshot)
    } else {
        Err(ApplicationError::AdminMail(AdminMailError::InvalidProjection))
    }
}

pub(super) async fn list_intakes_postgres(
    store: &crate::database::PostgresStore,
    request: AdminMailIntakeFilter,
) -> Result<AdminMailIntakePage, ApplicationError> {
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启邮件入站读取事务失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("设置邮件入站读取隔离级别失败"))?;

    let mailbox_key = request.mailbox_key.as_deref();
    let world_id = request.world_id.as_deref();
    let kind = request.kind.as_deref();
    let status = request.status.as_deref();
    let decision = request.decision.as_deref();
    let received_from = request.received_from;
    let received_to = request.received_to;
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.inbound_mail
         WHERE ($1::text IS NULL OR mailbox_key = $1)
           AND ($2::text IS NULL OR world_id = $2)
           AND ($3::text IS NULL OR mail_kind = $3)
           AND ($4::text IS NULL OR status = $4)
           AND ($5::text IS NULL OR decision_code = $5)
           AND ($6::timestamptz IS NULL OR received_at >= $6)
           AND ($7::timestamptz IS NULL OR received_at <= $7)",
    )
    .bind(mailbox_key)
    .bind(world_id)
    .bind(kind)
    .bind(status)
    .bind(decision)
    .bind(received_from)
    .bind(received_to)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|_| storage_error("统计邮件入站记录失败"))?;
    let rows = sqlx::query(
        "SELECT id, mailbox_key, external_mail_id, world_id, mail_kind, status,
                decision_code, received_at, processed_at
         FROM public.inbound_mail
         WHERE ($1::text IS NULL OR mailbox_key = $1)
           AND ($2::text IS NULL OR world_id = $2)
           AND ($3::text IS NULL OR mail_kind = $3)
           AND ($4::text IS NULL OR status = $4)
           AND ($5::text IS NULL OR decision_code = $5)
           AND ($6::timestamptz IS NULL OR received_at >= $6)
           AND ($7::timestamptz IS NULL OR received_at <= $7)
         ORDER BY received_at DESC, id DESC
         LIMIT $8 OFFSET $9",
    )
    .bind(mailbox_key)
    .bind(world_id)
    .bind(kind)
    .bind(status)
    .bind(decision)
    .bind(received_from)
    .bind(received_to)
    .bind(request.page_size)
    .bind(request.offset())
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取邮件入站列表失败"))?;
    let items =
        rows.into_iter().map(|row| map_intake_summary(&row)).collect::<Result<Vec<_>, _>>()?;
    transaction.commit().await.map_err(|_| storage_error("提交邮件入站读取事务失败"))?;
    let page =
        AdminMailIntakePage { items, total, page: request.page, page_size: request.page_size };
    if page.validate(&request) {
        Ok(page)
    } else {
        Err(ApplicationError::AdminMail(AdminMailError::InvalidProjection))
    }
}

pub(super) async fn get_intake_postgres(
    store: &crate::database::PostgresStore,
    intake_id: Uuid,
) -> Result<Option<AdminMailIntakeDetail>, ApplicationError> {
    let row = sqlx::query(
        "SELECT id, mailbox_key, external_mail_id, world_id, mail_kind, status,
                decision_code, received_at, processed_at, decision_metadata
         FROM public.inbound_mail WHERE id = $1",
    )
    .bind(intake_id)
    .fetch_optional(store.pool())
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 邮件入站详情失败"))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let summary = map_intake_summary(&row)?;
    let decision_metadata: Json<Value> =
        row.try_get("decision_metadata").map_err(|_| storage_error("读取邮件决定元数据失败"))?;
    let metadata = super::mapping::decode_metadata(decision_metadata.0)?;
    if !summary.validate() || !metadata.validate() {
        return Err(ApplicationError::AdminMail(AdminMailError::InvalidProjection));
    }
    Ok(Some(AdminMailIntakeDetail { summary, metadata }))
}
