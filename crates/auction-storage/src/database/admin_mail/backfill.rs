//! PostgreSQL MoogleMail 展示投影的安全历史回填。

use auction_application::{AdminMailError, ApplicationError, MailDisplaySnapshot};
use serde_json::Value;
use sqlx::{Row, types::Json};
use uuid::Uuid;

use super::{DatabaseStore, mapping};
use crate::database::audit_health::AuditWriteResult as _;

/// 经过 authenticated parsed_message 证据校验的邮件展示回填输入。
#[derive(Debug, Clone)]
pub struct MailDisplayBackfillInput {
    pub mailbox_key: String,
    pub world_id: String,
    pub external_mail_id: String,
    pub content_digest: [u8; 32],
    pub sender: String,
    pub subject: String,
    pub message_display: MailDisplaySnapshot,
    pub evidence_id: Uuid,
}

/// 展示回填结果；不包含正文或 key。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailDisplayBackfillResult {
    pub intake_id: Uuid,
    pub updated: bool,
}

impl DatabaseStore {
    /// 仅将已认证证据生成的展示投影从 NULL 原子回填，绝不改变业务决定或游标。
    pub async fn backfill_mail_display(
        &self,
        input: MailDisplayBackfillInput,
    ) -> Result<MailDisplayBackfillResult, ApplicationError> {
        let store = match self {
            DatabaseStore::Postgres(store) => store,
            #[cfg(feature = "sqlite")]
            DatabaseStore::Sqlite(_) => {
                return Err(ApplicationError::CapabilityUnavailable(
                    "MoogleMail 展示回填仅支持 PostgreSQL 完整档",
                ));
            }
        };
        if input.evidence_id.is_nil()
            || input.content_digest == [0; 32]
            || !valid_header_text(&input.mailbox_key, 256)
            || !matches!(input.world_id.as_str(), "main" | "isekai")
            || !valid_header_text(&input.world_id, 16)
            || !valid_header_text(&input.external_mail_id, 512)
            || !valid_header_text(&input.sender, 256)
            || !valid_header_text(&input.subject, 512)
            || !input.message_display.validate()
        {
            return Err(ApplicationError::AdminMail(AdminMailError::InvalidRequest));
        }
        let mut transaction = store
            .pool()
            .begin()
            .await
            .map_err(|_| mapping::storage_error("开启邮件展示回填事务失败"))?;
        let row = sqlx::query(
            "SELECT id, world_id, content_digest, sender, subject, message_display
             FROM public.inbound_mail
             WHERE mailbox_key = $1 AND external_mail_id = $2
             FOR UPDATE",
        )
        .bind(&input.mailbox_key)
        .bind(&input.external_mail_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| mapping::storage_error("读取邮件展示回填目标失败"))?
        .ok_or_else(|| mapping::storage_error("邮件展示回填目标不存在"))?;
        let intake_id: Uuid =
            row.try_get("id").map_err(|_| mapping::storage_error("读取邮件展示回填 ID 失败"))?;
        let world_id: String = row
            .try_get("world_id")
            .map_err(|_| mapping::storage_error("读取邮件展示回填世界失败"))?;
        let digest: Vec<u8> = row
            .try_get("content_digest")
            .map_err(|_| mapping::storage_error("读取邮件展示回填摘要失败"))?;
        let sender: Option<String> = row
            .try_get("sender")
            .map_err(|_| mapping::storage_error("读取邮件展示回填发件人失败"))?;
        let subject: Option<String> = row
            .try_get("subject")
            .map_err(|_| mapping::storage_error("读取邮件展示回填标题失败"))?;
        let existing: Option<Json<Value>> = row
            .try_get("message_display")
            .map_err(|_| mapping::storage_error("读取邮件展示回填投影失败"))?;
        if world_id != input.world_id
            || digest.as_slice() != input.content_digest
            || sender.as_deref().is_some_and(|value| value != input.sender)
            || subject.as_deref().is_some_and(|value| value != input.subject)
            || sender.is_some() != subject.is_some()
        {
            return Err(mapping::storage_error("邮件展示回填身份或摘要冲突"));
        }
        let expected = serde_json::to_value(&input.message_display)
            .map_err(|_| mapping::storage_error("序列化邮件展示回填失败"))?;
        let updated = match (sender, subject, existing) {
            (None, None, None) => {
                let result = sqlx::query(
                    "UPDATE public.inbound_mail
                     SET sender = $1, subject = $2, message_display = $3
                     WHERE id = $4 AND sender IS NULL AND subject IS NULL AND message_display IS NULL",
                )
                .bind(&input.sender)
                .bind(&input.subject)
                .bind(Json(expected.clone()))
                .bind(intake_id)
                .execute(&mut *transaction)
                .await
                .map_err(|_| mapping::storage_error("写入邮件展示回填失败"))?;
                if result.rows_affected() != 1 {
                    return Err(mapping::storage_error("邮件展示回填 CAS 冲突"));
                }
                true
            }
            (Some(existing_sender), Some(existing_subject), Some(existing_display))
                if existing_sender == input.sender
                    && existing_subject == input.subject
                    && existing_display.0 == expected =>
            {
                false
            }
            _ => return Err(mapping::storage_error("邮件展示回填值冲突")),
        };
        let audit_id = Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("auction/mail-display-backfill-v1/{}", input.evidence_id).as_bytes(),
        );
        sqlx::query(
            "INSERT INTO public.audit_records
                (id, actor_type, actor_id, action, entity_type, entity_id, result, request_id, source, metadata)
             VALUES ($1, 'system', 'auctionctl', 'mail.display_backfill', 'inbound_mail', $2,
                     'success', $3, 'system', $4)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(audit_id)
        .bind(intake_id.to_string())
        .bind(input.evidence_id.to_string())
        .bind(Json(serde_json::json!({ "evidence_id": input.evidence_id })))
        .execute(&mut *transaction)
        .await
        .record_audit_failure()
        .map_err(|_| mapping::storage_error("追加邮件展示回填审计失败"))?;
        transaction
            .commit()
            .await
            .map_err(|_| mapping::storage_error("提交邮件展示回填事务失败"))?;
        Ok(MailDisplayBackfillResult { intake_id, updated })
    }
}

fn valid_header_text(value: &str, maximum_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum_bytes
        && value == value.trim()
        && !value.chars().any(char::is_control)
}
