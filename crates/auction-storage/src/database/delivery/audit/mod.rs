//! PostgreSQL Delivery owner 的同事务追加式审计写入；只接受已校验的低基数状态元数据，
//! payload、远端原文、Cookie 和密钥永不入审计，写入失败必须回滚调用方事务。

use crate::database::TrackedTransaction as Transaction;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::Postgres;
use uuid::Uuid;

use crate::database::operations::audit_health::AuditWriteResult as _;

pub(in crate::database) const ACTIVATE: &str = "delivery.activate";
pub(in crate::database) const ADMIN_RECONCILE: &str = "delivery.admin_reconcile";
pub(in crate::database) const ADMIN_RELEASE: &str = "delivery.admin_release";
pub(in crate::database) const ADMIN_RETRY_FAILED: &str = "delivery.admin_retry_failed";
pub(in crate::database) const CLAIM: &str = "delivery.claim";
/// 确认原退件已取回后重排同一 COD；保留被清理的旧结果及消费身份。
pub(in crate::database) const COD_RETURN_RESEND: &str = "delivery.cod_return_resend";
pub(in crate::database) const CREATE: &str = "delivery.create";
pub(in crate::database) const FAIL: &str = "delivery.fail";
pub(in crate::database) const OPERATOR_ACKNOWLEDGED: &str = "delivery.operator_acknowledged";
pub(in crate::database) const OUTCOME_UNKNOWN: &str = "delivery.outcome_unknown";
pub(in crate::database) const PREPARE: &str = "delivery.prepare";
pub(in crate::database) const QUARANTINE_MAIL_DISPOSITION: &str =
    "delivery.quarantine_mail_disposition";
pub(in crate::database) const RECONCILE_EXPIRED_SENDING: &str =
    "delivery.reconcile_expired_sending";
pub(in crate::database) const RELEASE: &str = "delivery.release";
pub(in crate::database) const START_SENDING: &str = "delivery.start_sending";
pub(in crate::database) const SUCCEED: &str = "delivery.succeed";

pub(in crate::database) struct DeliveryAudit<'a> {
    pub occurred_at: DateTime<Utc>,
    pub actor_type: &'static str,
    pub actor_id: &'a str,
    pub action: &'static str,
    pub delivery_id: Uuid,
    pub request_id: Option<&'a str>,
    pub source: &'static str,
    pub metadata: Value,
}

/// 在 Delivery 状态事务中追加一条不可变安全审计，并返回该审计 ID。
///
/// 需要以审计本身作为处置/证明来源引用的调用方使用返回值；其余调用方可以忽略。
pub(in crate::database) async fn append(
    transaction: &mut Transaction<'_, Postgres>,
    audit: DeliveryAudit<'_>,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO public.audit_records
               (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
                result, request_id, source, metadata)
           VALUES ($1, $2, $3, $4, $5, 'delivery', $6, 'success', $7, $8, $9)"#,
    )
    .bind(id)
    .bind(audit.occurred_at)
    .bind(audit.actor_type)
    .bind(audit.actor_id)
    .bind(audit.action)
    .bind(audit.delivery_id.to_string())
    .bind(audit.request_id)
    .bind(audit.source)
    .bind(audit.metadata)
    .execute(&mut **transaction)
    .await
    .record_audit_failure()?;
    Ok(id)
}

pub(in crate::database) fn worker_state_metadata(
    status: &str,
    fencing_version: i64,
    attempt_count: u32,
    failure_code: Option<&str>,
) -> Value {
    let mut metadata = serde_json::json!({
        "status": status,
        "fencing_version": fencing_version,
        "attempt_count": attempt_count,
    });
    if let (Some(object), Some(failure_code)) = (metadata.as_object_mut(), failure_code) {
        object.insert("failure_code".to_owned(), failure_code.into());
    }
    metadata
}
