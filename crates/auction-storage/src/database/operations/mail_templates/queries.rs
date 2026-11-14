//! 每个模板键的最新修订事实；键是否属于本版闭集由应用层判断。

use auction_application::mail_templates::{MailTemplateEvent, MailTemplateStoreError};
use sqlx::{Row, postgres::PgRow};

use crate::database::DatabaseStore;

pub(super) const LATEST: &str = "SELECT * FROM public.mail_template_events WHERE template_key=$1 ORDER BY revision DESC LIMIT 1";

pub(super) async fn latest(
    store: &DatabaseStore,
) -> Result<Vec<MailTemplateEvent>, MailTemplateStoreError> {
    // 表里只有被修订过的模板键；DISTINCT ON 走 (template_key, revision DESC) 索引，每键取一条。
    let rows = sqlx::query(
        "SELECT DISTINCT ON (template_key) *
         FROM public.mail_template_events
         ORDER BY template_key, revision DESC",
    )
    .fetch_all(store.pool())
    .await
    .map_err(unavailable)?;
    rows.iter().map(map).collect()
}

pub(super) fn map(row: &PgRow) -> Result<MailTemplateEvent, MailTemplateStoreError> {
    let decode = || -> Result<MailTemplateEvent, sqlx::Error> {
        let operation: String = row.try_get("operation")?;
        Ok(MailTemplateEvent {
            command_id: row.try_get("command_id")?,
            template_key: row.try_get("template_key")?,
            revision: row.try_get("revision")?,
            operation: serde_json::from_value(serde_json::Value::String(operation))
                .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
            body: row.try_get("body")?,
            actor_id: row.try_get("actor_id")?,
            recorded_at: row.try_get("recorded_at")?,
        })
    };
    let event = decode().map_err(corrupt)?;
    event.validate()?;
    // 固定结果与事实列必须一致：重放返回的就是这份结果。
    let result: serde_json::Value = row.try_get("result_payload").map_err(corrupt)?;
    if serde_json::to_value(&event).map_err(corrupt)? != result {
        return Err(MailTemplateStoreError::Corrupt);
    }
    Ok(event)
}

fn corrupt<T>(_: T) -> MailTemplateStoreError {
    MailTemplateStoreError::Corrupt
}

fn unavailable<T>(_: T) -> MailTemplateStoreError {
    MailTemplateStoreError::Unavailable
}
