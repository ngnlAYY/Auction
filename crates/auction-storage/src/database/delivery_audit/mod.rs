//! PostgreSQL Delivery owner 的同事务追加式审计写入。
//!
//! 这里只接受调用方已经校验过的低基数状态元数据；delivery payload、远端原文、Cookie 和密钥
//! 永远不能进入审计记录。任何审计写入失败都会使调用方事务回滚，避免状态事实与审计证据分叉。

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::database::audit_health::AuditWriteResult as _;

pub(super) const ACTIVATE: &str = "delivery.activate";
pub(super) const ADMIN_RECONCILE: &str = "delivery.admin_reconcile";
pub(super) const ADMIN_RELEASE: &str = "delivery.admin_release";
pub(super) const ADMIN_RETRY_FAILED: &str = "delivery.admin_retry_failed";
pub(super) const CLAIM: &str = "delivery.claim";
pub(super) const CREATE: &str = "delivery.create";
pub(super) const FAIL: &str = "delivery.fail";
pub(super) const OUTCOME_UNKNOWN: &str = "delivery.outcome_unknown";
pub(super) const PREPARE: &str = "delivery.prepare";
pub(super) const RECONCILE_EXPIRED_SENDING: &str = "delivery.reconcile_expired_sending";
pub(super) const RELEASE: &str = "delivery.release";
pub(super) const START_SENDING: &str = "delivery.start_sending";
pub(super) const SUCCEED: &str = "delivery.succeed";

pub(super) struct DeliveryAudit<'a> {
    pub occurred_at: DateTime<Utc>,
    pub actor_type: &'static str,
    pub actor_id: &'a str,
    pub action: &'static str,
    pub delivery_id: Uuid,
    pub request_id: Option<&'a str>,
    pub source: &'static str,
    pub metadata: Value,
}

/// 在 Delivery 状态事务中追加一条不可变安全审计。
pub(super) async fn append(
    transaction: &mut Transaction<'_, Postgres>,
    audit: DeliveryAudit<'_>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"INSERT INTO public.audit_records
               (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
                result, request_id, source, metadata)
           VALUES ($1, $2, $3, $4, $5, 'delivery', $6, 'success', $7, $8, $9)"#,
    )
    .bind(Uuid::new_v4())
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
    Ok(())
}

pub(super) fn worker_state_metadata(
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
