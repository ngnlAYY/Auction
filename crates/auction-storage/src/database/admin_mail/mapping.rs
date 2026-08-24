//! PostgreSQL Admin 邮件行、轮询状态和安全 metadata 映射。

use super::*;

pub(super) fn map_sql_error(error: sqlx::Error) -> AdminMailTransactionError {
    match &error {
        sqlx::Error::Database(database_error)
            if database_error.code().as_deref() == Some("40001") =>
        {
            AdminMailTransactionError::SerializationConflict
        }
        sqlx::Error::Database(database_error)
            if database_error.code().as_deref() == Some("23505") =>
        {
            AdminMailTransactionError::Conflict
        }
        _ => AdminMailTransactionError::Failed("PostgreSQL 邮件 Admin 事务操作失败".to_owned()),
    }
}

pub(super) fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(auction_application::RepositoryError::Failed(message.to_owned()))
}

pub(super) fn map_poll_state(
    row: sqlx::postgres::PgRow,
) -> Result<AdminMailPollState, AdminMailTransactionError> {
    Ok(AdminMailPollState {
        mailbox_key: row.try_get("mailbox_key").map_err(map_sql_error)?,
        cursor_value: row.try_get("cursor_value").map_err(map_sql_error)?,
        last_poll_at: row.try_get("last_poll_at").map_err(map_sql_error)?,
        next_poll_at: row.try_get("next_poll_at").map_err(map_sql_error)?,
        consecutive_failures: row.try_get("consecutive_failures").map_err(map_sql_error)?,
        paused: row.try_get("paused").map_err(map_sql_error)?,
        state_version: row.try_get("state_version").map_err(map_sql_error)?,
        updated_at: row.try_get("updated_at").map_err(map_sql_error)?,
    })
}

pub(super) fn map_mail_poller(
    row: &sqlx::postgres::PgRow,
) -> Result<AdminMailPollerSnapshot, ApplicationError> {
    Ok(AdminMailPollerSnapshot {
        mailbox_key: row
            .try_get("mailbox_key")
            .map_err(|_| storage_error("读取邮件轮询器邮箱标识失败"))?,
        last_poll_at: row
            .try_get("last_poll_at")
            .map_err(|_| storage_error("读取邮件轮询器上次轮询时间失败"))?,
        next_poll_at: row
            .try_get("next_poll_at")
            .map_err(|_| storage_error("读取邮件轮询器下次轮询时间失败"))?,
        consecutive_failures: row
            .try_get("consecutive_failures")
            .map_err(|_| storage_error("读取邮件轮询器失败计数失败"))?,
        paused: row.try_get("paused").map_err(|_| storage_error("读取邮件轮询器暂停状态失败"))?,
        state_version: row
            .try_get("state_version")
            .map_err(|_| storage_error("读取邮件轮询器状态版本失败"))?,
        updated_at: row
            .try_get("updated_at")
            .map_err(|_| storage_error("读取邮件轮询器更新时间失败"))?,
    })
}

pub(super) fn map_intake_summary(
    row: &sqlx::postgres::PgRow,
) -> Result<AdminMailIntakeSummary, ApplicationError> {
    let summary = AdminMailIntakeSummary {
        id: row.try_get("id").map_err(|_| storage_error("读取邮件入站标识失败"))?,
        mailbox_key: row
            .try_get("mailbox_key")
            .map_err(|_| storage_error("读取邮件邮箱标识失败"))?,
        external_mail_id: row
            .try_get("external_mail_id")
            .map_err(|_| storage_error("读取外部邮件标识失败"))?,
        world_id: row.try_get("world_id").map_err(|_| storage_error("读取邮件世界标识失败"))?,
        kind: row.try_get("mail_kind").map_err(|_| storage_error("读取邮件类型失败"))?,
        status: row.try_get("status").map_err(|_| storage_error("读取邮件状态失败"))?,
        decision_code: row
            .try_get("decision_code")
            .map_err(|_| storage_error("读取邮件决定码失败"))?,
        received_at: row
            .try_get("received_at")
            .map_err(|_| storage_error("读取邮件接收时间失败"))?,
        processed_at: row
            .try_get("processed_at")
            .map_err(|_| storage_error("读取邮件处理时间失败"))?,
    };
    if summary.validate() {
        Ok(summary)
    } else {
        Err(storage_error("邮件入站摘要安全投影无效"))
    }
}

#[derive(Debug, Default, Deserialize)]
struct DecisionMetadata {
    auction_id: Option<String>,
    submission_id: Option<String>,
    user_id: Option<String>,
    added_count: Option<i64>,
    existing_count: Option<i64>,
    preview_revision: Option<i64>,
    attachment_count: Option<i64>,
    owner_result_code: Option<String>,
    reason_code: Option<String>,
    warning_codes: Option<Vec<String>>,
    delivery_ids: Option<Vec<String>>,
}

pub(super) fn decode_metadata(value: Value) -> Result<AdminMailIntakeMetadata, ApplicationError> {
    let value = serde_json::from_value::<DecisionMetadata>(value)
        .map_err(|_| storage_error("邮件决定元数据格式无效"))?;
    let added_count = to_i32(value.added_count.unwrap_or_default())?;
    let existing_count = to_i32(value.existing_count.unwrap_or_default())?;
    let attachment_count = to_i32(value.attachment_count.unwrap_or_default())?;
    let metadata = AdminMailIntakeMetadata {
        auction_id: value.auction_id.unwrap_or_default(),
        submission_id: value.submission_id.unwrap_or_default(),
        user_id: value.user_id.unwrap_or_default(),
        added_count,
        existing_count,
        preview_revision: value.preview_revision.unwrap_or_default(),
        attachment_count,
        owner_result_code: value.owner_result_code.unwrap_or_default(),
        reason_code: value.reason_code.unwrap_or_default(),
        warning_codes: value.warning_codes.unwrap_or_default(),
        delivery_ids: value.delivery_ids.unwrap_or_default(),
    };
    Ok(metadata)
}

fn to_i32(value: i64) -> Result<i32, ApplicationError> {
    i32::try_from(value).map_err(|_| storage_error("邮件决定计数超出安全范围"))
}
