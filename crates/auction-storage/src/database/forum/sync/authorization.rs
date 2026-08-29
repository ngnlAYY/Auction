//! 论坛未知写入的只读清单与人工重新写入授权。
//!
//! 人工命令只能从 `write_authorization_required` 推进到 `write_retry_authorized`；命令账本、
//! 状态更新和追加式审计在同一事务提交。真正的远端 POST 仍由 Worker 执行，并再次先查标题。

use auction_application::{
    ApplicationError, ForumSyncRecoveryItem, ForumWriteRetryAuthorizationCommand,
    ForumWriteRetryAuthorizationResult, RepositoryError,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use super::super::super::PostgresStore;

const COMMAND_KIND: &str = "authorize_write_retry";
const AUTHORIZATION_REQUIRED: &str = "write_authorization_required";
const WRITE_AUTHORIZED: &str = "write_retry_authorized";

pub(super) async fn list(
    store: &PostgresStore,
    limit: u16,
) -> Result<Vec<ForumSyncRecoveryItem>, ApplicationError> {
    let rows = sqlx::query(
        "SELECT auction_id, status, source_revision, attempt_count,
                CASE WHEN status = 'publishing' THEN 'write_unknown'
                     ELSE last_error_code END AS failure_code,
                next_attempt_at, updated_at
         FROM public.auction_forum_sync
         WHERE (status = 'publishing' AND lease_expires_at <= CURRENT_TIMESTAMP)
            OR last_error_code IN (
                'write_unknown', 'publish_proof_failed',
                'write_authorization_required', 'write_retry_authorized'
            )
            OR left(last_error_code, 10) = 'reconcile_'
         ORDER BY updated_at DESC, auction_id
         LIMIT $1",
    )
    .bind(i64::from(limit))
    .fetch_all(store.pool())
    .await
    .map_err(|_| storage_error("读取论坛未知写入恢复清单失败"))?;

    rows.into_iter().map(map_item).collect()
}

pub(super) async fn authorize(
    store: &PostgresStore,
    command: &ForumWriteRetryAuthorizationCommand,
) -> Result<ForumWriteRetryAuthorizationResult, ApplicationError> {
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启论坛写入授权事务失败"))?;
    if let Some(result) = existing_command(&mut transaction, command).await? {
        transaction.commit().await.map_err(|_| storage_error("提交论坛写入授权重放失败"))?;
        return Ok(result);
    }

    let row = sqlx::query(
        "SELECT status, source_revision, last_error_code, claim_token
         FROM public.auction_forum_sync
         WHERE auction_id = $1
         FOR UPDATE",
    )
    .bind(command.auction_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| storage_error("锁定论坛同步恢复状态失败"))?
    .ok_or_else(|| storage_error("论坛同步恢复状态不存在"))?;
    let status: String =
        row.try_get("status").map_err(|_| storage_error("读取论坛同步状态失败"))?;
    let failure_code: Option<String> =
        row.try_get("last_error_code").map_err(|_| storage_error("读取论坛同步失败码失败"))?;
    let claim_token: Option<Uuid> =
        row.try_get("claim_token").map_err(|_| storage_error("读取论坛同步租约失败"))?;
    if status != "failed"
        || failure_code.as_deref() != Some(AUTHORIZATION_REQUIRED)
        || claim_token.is_some()
    {
        // 并发的同一命令可能在等待行锁期间已经提交；此时仍允许稳定重放原结果。
        if let Some(result) = existing_command(&mut transaction, command).await? {
            transaction.commit().await.map_err(|_| storage_error("提交论坛写入授权重放失败"))?;
            return Ok(result);
        }
        return Err(storage_error("论坛同步当前不处于待人工写入授权状态"));
    }

    let updated = sqlx::query(
        "UPDATE public.auction_forum_sync
         SET status = 'failed',
             next_attempt_at = CURRENT_TIMESTAMP,
             last_error_code = $2,
             last_error_detail = '人工已授权再次尝试；Worker 仍须先按精确标题查找',
             updated_at = CURRENT_TIMESTAMP
         WHERE auction_id = $1
           AND status = 'failed'
           AND last_error_code = $3
           AND claim_token IS NULL
         RETURNING source_revision, updated_at",
    )
    .bind(command.auction_id)
    .bind(WRITE_AUTHORIZED)
    .bind(AUTHORIZATION_REQUIRED)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| storage_error("推进论坛写入授权状态失败"))?
    .ok_or_else(|| storage_error("论坛同步写入授权状态已变化"))?;
    let source_revision: i64 = updated
        .try_get("source_revision")
        .map_err(|_| storage_error("读取论坛写入授权源版本失败"))?;
    let authorized_at: DateTime<Utc> =
        updated.try_get("updated_at").map_err(|_| storage_error("读取论坛写入授权时间失败"))?;
    let result = ForumWriteRetryAuthorizationResult {
        command_id: command.command_id,
        auction_id: command.auction_id,
        source_revision,
        authorized_at,
    };
    let payload =
        serde_json::to_value(&result).map_err(|_| storage_error("编码论坛写入授权结果失败"))?;
    let inserted = sqlx::query(
        "INSERT INTO public.forum_sync_admin_commands
             (command_id, command_kind, request_digest, auction_id, actor_id, result_payload)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (command_id) DO NOTHING",
    )
    .bind(command.command_id)
    .bind(COMMAND_KIND)
    .bind(command.request_hash.as_bytes().to_vec())
    .bind(command.auction_id)
    .bind(&command.actor_id)
    .bind(payload)
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("写入论坛写入授权命令账本失败"))?;
    if inserted.rows_affected() != 1 {
        return Err(storage_error("论坛写入授权命令发生并发冲突"));
    }
    sqlx::query(
        "INSERT INTO public.audit_records
             (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
              result, request_id, source, metadata)
         VALUES ($1, $2, 'admin', $3, 'forum.authorize_write_retry',
                 'auction_forum_sync', $4, 'success', $5, 'admin', $6)",
    )
    .bind(Uuid::new_v4())
    .bind(authorized_at)
    .bind(&command.actor_id)
    .bind(command.auction_id.to_string())
    .bind(command.command_id.to_string())
    .bind(serde_json::json!({
        "command_id": command.command_id,
        "previous_failure_code": AUTHORIZATION_REQUIRED,
        "next_failure_code": WRITE_AUTHORIZED,
        "source_revision": source_revision,
    }))
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("追加论坛写入授权审计失败"))?;
    transaction.commit().await.map_err(|_| storage_error("提交论坛写入授权事务失败"))?;
    Ok(result)
}

async fn existing_command(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &ForumWriteRetryAuthorizationCommand,
) -> Result<Option<ForumWriteRetryAuthorizationResult>, ApplicationError> {
    let Some(row) = sqlx::query(
        "SELECT command_kind, request_digest, auction_id, actor_id, result_payload
         FROM public.forum_sync_admin_commands
         WHERE command_id = $1",
    )
    .bind(command.command_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| storage_error("读取论坛写入授权命令账本失败"))?
    else {
        return Ok(None);
    };
    let command_kind: String =
        row.try_get("command_kind").map_err(|_| storage_error("读取论坛写入授权命令类型失败"))?;
    let request_digest: Vec<u8> =
        row.try_get("request_digest").map_err(|_| storage_error("读取论坛写入授权命令摘要失败"))?;
    let auction_id: Uuid =
        row.try_get("auction_id").map_err(|_| storage_error("读取论坛写入授权拍卖标识失败"))?;
    let actor_id: String =
        row.try_get("actor_id").map_err(|_| storage_error("读取论坛写入授权操作者失败"))?;
    if command_kind != COMMAND_KIND
        || request_digest != command.request_hash.as_bytes()
        || auction_id != command.auction_id
        || actor_id != command.actor_id
    {
        return Err(storage_error("论坛写入授权命令 ID 与已有请求冲突"));
    }
    let payload: Value =
        row.try_get("result_payload").map_err(|_| storage_error("读取论坛写入授权结果失败"))?;
    let result: ForumWriteRetryAuthorizationResult =
        serde_json::from_value(payload).map_err(|_| storage_error("论坛写入授权命令结果损坏"))?;
    if result.command_id != command.command_id || result.auction_id != command.auction_id {
        return Err(storage_error("论坛写入授权命令结果身份不一致"));
    }
    Ok(Some(result))
}

fn map_item(row: sqlx::postgres::PgRow) -> Result<ForumSyncRecoveryItem, ApplicationError> {
    let attempt_count: i32 =
        row.try_get("attempt_count").map_err(|_| storage_error("读取论坛恢复尝试次数失败"))?;
    let item = ForumSyncRecoveryItem {
        auction_id: row
            .try_get("auction_id")
            .map_err(|_| storage_error("读取论坛恢复拍卖标识失败"))?,
        status: row.try_get("status").map_err(|_| storage_error("读取论坛恢复状态失败"))?,
        source_revision: row
            .try_get("source_revision")
            .map_err(|_| storage_error("读取论坛恢复源版本失败"))?,
        attempt_count: u32::try_from(attempt_count)
            .map_err(|_| storage_error("论坛恢复尝试次数无效"))?,
        failure_code: row
            .try_get("failure_code")
            .map_err(|_| storage_error("读取论坛恢复失败码失败"))?,
        next_attempt_at: row
            .try_get("next_attempt_at")
            .map_err(|_| storage_error("读取论坛恢复下次尝试时间失败"))?,
        updated_at: row
            .try_get("updated_at")
            .map_err(|_| storage_error("读取论坛恢复更新时间失败"))?,
    };
    if item.auction_id.is_nil()
        || item.source_revision <= 0
        || item.failure_code.is_empty()
        || item.failure_code != item.failure_code.trim()
        || item.next_attempt_at.timestamp_nanos_opt().is_none()
        || item.updated_at.timestamp_nanos_opt().is_none()
    {
        return Err(storage_error("论坛恢复投影无效"));
    }
    Ok(item)
}

fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}
