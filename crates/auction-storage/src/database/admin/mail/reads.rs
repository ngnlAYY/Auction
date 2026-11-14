//! PostgreSQL Admin 邮件轮询状态、dashboard 与入站记录查询。

use auction_application::admin::{
    AdminMailDashboardSnapshot, AdminMailError, AdminMailIntakeDetail, AdminMailIntakeFilter,
    AdminMailIntakePage, AdminMailPollState, AdminMailPollStateRequest, AdminMailPollerSnapshot,
    AdminMailTransactionError, MAX_ADMIN_MAIL_POLLER_COUNT,
};
use auction_application::core::ApplicationError;
use chrono::Utc;
use serde_json::Value;
use sqlx::Row;
use sqlx::types::Json;
use uuid::Uuid;

use super::mapping::{map_intake_delivery, map_intake_summary, map_mail_poller, map_poll_state};
use crate::database::storage_error_from;

pub(super) async fn get_poll_state_postgres(
    store: &crate::database::DatabaseStore,
    request: AdminMailPollStateRequest,
) -> Result<AdminMailPollState, ApplicationError> {
    let row = sqlx::query(
        "SELECT mailbox_key, cursor_value, last_poll_at, next_poll_at,
                consecutive_failures, paused, state_version, updated_at,
                CASE WHEN paused THEN (
                    SELECT command.created_at FROM public.mail_admin_commands AS command
                    WHERE command.mailbox_key = mail_poll_state.mailbox_key
                      AND (command.result_payload ->> 'paused')::boolean
                    ORDER BY command.created_at DESC LIMIT 1
                ) END AS paused_since
         FROM public.mail_poll_state WHERE mailbox_key = $1",
    )
    .bind(request.mailbox_key)
    .fetch_optional(store.pool())
    .await
    .map_err(|error| storage_error_from(error, "读取 PostgreSQL 邮件轮询状态失败"))?;
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
    store: &crate::database::DatabaseStore,
) -> Result<AdminMailDashboardSnapshot, ApplicationError> {
    // 轮询器数量是固定的部署状态集合；LIMIT + 1 既防膨胀表无限进入响应，又能报告超出安全投影上限。
    let rows = sqlx::query(
        "SELECT state.mailbox_key, state.last_poll_at, state.next_poll_at,
                state.consecutive_failures, state.paused, state.state_version, state.updated_at,
                CASE WHEN state.paused THEN (
                    SELECT command.created_at FROM public.mail_admin_commands AS command
                    WHERE command.mailbox_key = state.mailbox_key
                      AND (command.result_payload ->> 'paused')::boolean
                    ORDER BY command.created_at DESC LIMIT 1
                ) END AS paused_since
         FROM public.mail_poll_state AS state
         ORDER BY state.mailbox_key ASC
         LIMIT 65",
    )
    .fetch_all(store.pool())
    .await
    .map_err(|error| storage_error_from(error, "读取 PostgreSQL 邮件轮询器 dashboard 摘要失败"))?;
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
    store: &crate::database::DatabaseStore,
    request: AdminMailIntakeFilter,
) -> Result<AdminMailIntakePage, ApplicationError> {
    // 与 delivery 列表共用只读一致快照，保留 total/当前页原子投影，不参与写事务的 SSI。
    let mut transaction = store
        .pool()
        .begin_read_snapshot()
        .await
        .map_err(|error| storage_error_from(error, "开启邮件入站读取事务失败"))?;

    // 可选空串与省略具有相同筛选语义，不能绑定为 column = ''。
    let mailbox_key = request.mailbox_key.as_deref().filter(|value| !value.is_empty());
    let world_id = request.world_id.as_deref().filter(|value| !value.is_empty());
    let kind = request.kind.as_deref().filter(|value| !value.is_empty());
    let status = request.status.as_deref().filter(|value| !value.is_empty());
    let decision = request.decision.as_deref().filter(|value| !value.is_empty());
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
    .map_err(|error| storage_error_from(error, "统计邮件入站记录失败"))?;
    let rows = sqlx::query(
        "SELECT id, mailbox_key, external_mail_id, world_id, mail_kind, status,
                decision_code, received_at, processed_at, sender, subject,
                (message_display IS NOT NULL) AS has_message_display
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
    .map_err(|error| storage_error_from(error, "读取邮件入站列表失败"))?;
    let items =
        rows.into_iter().map(|row| map_intake_summary(&row)).collect::<Result<Vec<_>, _>>()?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交邮件入站读取事务失败"))?;
    let page =
        AdminMailIntakePage { items, total, page: request.page, page_size: request.page_size };
    if page.validate(&request) {
        Ok(page)
    } else {
        Err(ApplicationError::AdminMail(AdminMailError::InvalidProjection))
    }
}

pub(super) async fn get_intake_postgres(
    store: &crate::database::DatabaseStore,
    intake_id: Uuid,
) -> Result<Option<AdminMailIntakeDetail>, ApplicationError> {
    let row = sqlx::query(
        "SELECT id, mailbox_key, external_mail_id, world_id, mail_kind, status,
                decision_code, received_at, processed_at, sender, subject,
                (message_display IS NOT NULL) AS has_message_display,
                message_display, equipment_diagnostic, decision_metadata
         FROM public.inbound_mail WHERE id = $1",
    )
    .bind(intake_id)
    .fetch_optional(store.pool())
    .await
    .map_err(|error| storage_error_from(error, "读取 PostgreSQL 邮件入站详情失败"))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let summary = map_intake_summary(&row)?;
    let decision_metadata: Json<Value> = row
        .try_get("decision_metadata")
        .map_err(|error| storage_error_from(error, "读取邮件决定元数据失败"))?;
    let metadata = super::mapping::decode_metadata(decision_metadata.0)?;
    if !summary.validate() || !metadata.validate() {
        return Err(ApplicationError::AdminMail(AdminMailError::InvalidProjection));
    }
    let delivery_ids = metadata
        .delivery_ids
        .iter()
        .filter_map(|value| Uuid::parse_str(value).ok())
        .collect::<Vec<_>>();
    let deliveries = if delivery_ids.is_empty() {
        Vec::new()
    } else {
        let rows = sqlx::query(
            "SELECT id, payload_operation AS operation, recipient, status,
                    fencing_version, attempt_count, failure_code, external_reference,
                    result_metadata ->> 'reason_code' AS reason_code
             FROM public.deliveries
             WHERE origin_kind = 'mail_intake'
               AND id = ANY($1::uuid[])
               AND payload_world = $2
               AND payload ->> 'external_mail_id' = $3
               AND payload_operation IN ('send', 'return', 'take')
               AND (
                   origin_id = concat($2, ':', $3, ':', payload_operation)
                   OR (payload_operation = 'send'
                       AND origin_id = concat($2, ':', $3, ':delivery-notice'))
                   OR (payload_operation = 'return'
                       AND origin_id = concat($2, ':', $3, ':delivery-return'))
                   OR (payload_operation = 'take'
                       AND origin_id = concat($2, ':', $3, ':delivery-take'))
               )
             ORDER BY id",
        )
        .bind(delivery_ids)
        .bind(&summary.world_id)
        .bind(&summary.external_mail_id)
        .fetch_all(store.pool())
        .await
        .map_err(|error| storage_error_from(error, "读取邮件关联 delivery 失败"))?;
        rows.iter().map(map_intake_delivery).collect::<Result<Vec<_>, _>>()?
    };
    let message_display = row
        .try_get::<Option<Json<Value>>, _>("message_display")
        .map_err(|error| storage_error_from(error, "读取邮件原始信息失败"))?
        .map(|value| super::mapping::decode_message_display(value.0))
        .transpose()?;
    let equipment_diagnostic = row
        .try_get::<Option<Json<Value>>, _>("equipment_diagnostic")
        .map_err(|error| storage_error_from(error, "读取邮件装备诊断失败"))?
        .map(|value| super::mapping::decode_equipment_diagnostic(value.0))
        .transpose()?;
    if deliveries.iter().any(|value| !value.validate()) {
        return Err(ApplicationError::AdminMail(AdminMailError::InvalidProjection));
    }
    Ok(Some(AdminMailIntakeDetail {
        summary,
        metadata,
        deliveries,
        message_display,
        equipment_diagnostic,
    }))
}
