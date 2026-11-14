//! 命令锁 → 模板键锁 → 最新版本 CAS；事实、接收结果和追加审计同事务提交。

use auction_application::mail_templates::{
    MailTemplateCommand, MailTemplateEvent, MailTemplateStoreError,
};
use sqlx::Row;
use uuid::Uuid;

use super::queries;
use crate::database::{DatabaseStore, operations::audit_health::AuditWriteResult as _};

#[derive(Debug, thiserror::Error)]
pub(super) enum WriteError {
    #[error("邮件模板 SQL 失败")]
    Sql(#[from] sqlx::Error),
    #[error(transparent)]
    Application(#[from] MailTemplateStoreError),
}

impl WriteError {
    pub(super) fn public(self) -> MailTemplateStoreError {
        match self {
            Self::Application(error) => error,
            Self::Sql(_) => MailTemplateStoreError::Unavailable,
        }
    }
}

pub(super) async fn write(
    store: &DatabaseStore,
    command: &MailTemplateCommand,
) -> Result<MailTemplateEvent, WriteError> {
    let digest = command.digest()?;
    // 显式 Read Committed：等到模板键锁之后的语句取得最新已提交事实。
    let mut tx = store.pool().begin_with("BEGIN ISOLATION LEVEL READ COMMITTED").await?;
    crate::database::backend::lock_command_id(&mut tx, command.command_id).await?;
    if let Some(row) = sqlx::query("SELECT * FROM public.mail_template_events WHERE command_id=$1")
        .bind(command.command_id)
        .fetch_optional(&mut *tx)
        .await?
    {
        // 重放规则由应用命令定义：不重新核对当前版本，不按现行规则重审正文，也不追加第二份审计。
        return command
            .replay(
                row.try_get("digest_version")?,
                &row.try_get::<Vec<u8>, _>("request_digest")?,
                queries::map(&row)?,
            )
            .map_err(WriteError::Application);
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text,0))")
        .bind(format!("mail_template:{}", command.template_key))
        .execute(&mut *tx)
        .await?;
    let previous = sqlx::query(queries::LATEST)
        .bind(&command.template_key)
        .fetch_optional(&mut *tx)
        .await?
        .as_ref()
        .map(queries::map)
        .transpose()?;
    let now = sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(&mut *tx).await?;
    // 新命令在锁内完整校验：模板键闭集、正文规则与版本 CAS 都由应用命令判断。
    let event = command.accept(previous.as_ref(), now)?;
    let audit_id = Uuid::new_v4();
    // 审计只记身份与版本，不复制正文：正文在事实表里，审计表不应成为第二份模板存储。
    let metadata = serde_json::json!({
        "command_id": command.command_id,
        "template_key": event.template_key,
        "operation": event.operation.label(),
        "revision": event.revision,
        "previous_revision": previous.as_ref().map(|previous| previous.revision),
        "body_bytes": event.body.as_ref().map(String::len),
    });
    sqlx::query(
        "INSERT INTO public.audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id, result,
             request_id, source, metadata)
         VALUES ($1, $2, 'admin', $3, $4, 'mail_template', $5, 'success', $6, 'admin', $7)",
    )
    .bind(audit_id)
    .bind(now)
    .bind(&command.actor_id)
    .bind(format!("mail_template.{}", event.operation.label()))
    .bind(&event.template_key)
    .bind(command.request_id.as_deref())
    .bind(metadata)
    .execute(&mut *tx)
    .await
    .record_audit_failure()?;
    sqlx::query(
        "INSERT INTO public.mail_template_events
            (command_id, digest_version, request_digest, template_key, revision,
             expected_revision, operation, body, actor_id, audit_id, result_payload, recorded_at)
         VALUES ($1, 1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(command.command_id)
    .bind(digest.as_slice())
    .bind(&event.template_key)
    .bind(event.revision)
    .bind(command.expected_revision)
    .bind(event.operation.label())
    .bind(event.body.as_deref())
    .bind(&event.actor_id)
    .bind(audit_id)
    .bind(serde_json::to_value(&event).map_err(invalid)?)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    if let Err(error) = tx.commit().await {
        if crate::database::backend::observed_serialization_conflict(&error) {
            return Err(error.into());
        }
        // 提交回执丢失：事实可能已经落库，只能用原命令 ID 与原参数重试。
        return Err(MailTemplateStoreError::OutcomeUnknown.into());
    }
    Ok(event)
}

fn invalid<T>(_: T) -> MailTemplateStoreError {
    MailTemplateStoreError::Invalid
}
