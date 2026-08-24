//! Admin 凭据列表与命令账本读取。

use super::mapping::*;

use super::*;

#[cfg(feature = "postgres")]
pub(super) async fn list_postgres(
    store: &super::super::PostgresStore,
    request: AdminCredentialListRequest,
) -> Result<AdminCredentialPage, ApplicationError> {
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启凭据列表事务失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("设置凭据列表隔离级别失败"))?;

    let kind = request.kind.map(AdminCredentialKind::as_str);
    let (mail_scope, forum_scope) = world_scopes(request.world, request.kind);
    let mail_scope = mail_scope.as_deref();
    let forum_scope = forum_scope.as_deref();
    let status = request.status.map(AdminCredentialStatus::as_str);
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM public.credential_keys
         WHERE ($1::text IS NULL OR credential_kind = $1)
           AND ($2::text IS NULL OR scope = $2 OR scope = $3)
           AND ($4::text IS NULL OR status = $4)",
    )
    .bind(kind)
    .bind(mail_scope)
    .bind(forum_scope)
    .bind(status)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|_| storage_error("统计 Admin 凭据失败"))?;
    let rows = sqlx::query(
        "SELECT id, credential_kind, scope, label, status, state_version,
                created_at, rotated_at, updated_at
         FROM public.credential_keys
         WHERE ($1::text IS NULL OR credential_kind = $1)
           AND ($2::text IS NULL OR scope = $2 OR scope = $3)
           AND ($4::text IS NULL OR status = $4)
         ORDER BY created_at DESC, id DESC
         LIMIT $5 OFFSET $6",
    )
    .bind(kind)
    .bind(mail_scope)
    .bind(forum_scope)
    .bind(status)
    .bind(request.limit)
    .bind(request.offset)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 Admin 凭据列表失败"))?;
    transaction.commit().await.map_err(|_| storage_error("提交 Admin 凭据列表事务失败"))?;

    let credentials = rows.iter().map(map_postgres_credential).collect::<Result<Vec<_>, _>>()?;
    let page =
        AdminCredentialPage { credentials, total, limit: request.limit, offset: request.offset };
    page.validate(&request)
        .then_some(page)
        .ok_or_else(|| admin_error(AdminCredentialError::StoredResultCorrupt))
}

#[cfg(feature = "postgres")]
pub(super) async fn find_command_postgres(
    store: &super::super::PostgresStore,
    command_id: Uuid,
    kind: AdminCredentialCommandKind,
    request_hash: RequestHash,
) -> Result<Option<AdminCredentialCommandResult>, ApplicationError> {
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启凭据命令读取事务失败"))?;
    let row = sqlx::query(
        "SELECT command_kind, request_digest, credential_id, result_payload
         FROM public.credential_admin_commands
         WHERE command_id = $1
         FOR UPDATE",
    )
    .bind(command_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    let result = row.map(|row| parse_command(&row, kind, request_hash)).transpose()?;
    transaction.commit().await.map_err(|_| storage_error("提交凭据命令读取事务失败"))?;
    Ok(result)
}
