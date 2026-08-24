//! 凭据运行时读取、失效标记与健康投影。

#[cfg(feature = "postgres")]
use super::*;

#[cfg(feature = "postgres")]
pub(super) async fn list_active_postgres(
    store: &super::super::PostgresStore,
    kind: AdminCredentialKind,
    world: AdminCredentialWorld,
) -> Result<Vec<AdminCredentialRuntimeRecord>, ApplicationError> {
    let scope = world.scope(kind);
    let shared_scope = AdminCredentialWorld::Shared.scope(kind);
    let rows = sqlx::query(
        "SELECT id, scope, ciphertext, nonce, encryption_key_id, fingerprint, created_at
         FROM public.credential_keys
         WHERE credential_kind = $1
           AND status = 'active'
           AND scope IN ($2, $3)
         ORDER BY created_at ASC, id ASC",
    )
    .bind(kind.as_str())
    .bind(scope)
    .bind(shared_scope)
    .fetch_all(store.pool())
    .await
    .map_err(map_storage_error)?;
    rows.into_iter()
        .map(|row| {
            let envelope = AdminCredentialEnvelope::new(
                row.try_get("ciphertext").map_err(map_storage_error)?,
                row.try_get("nonce").map_err(map_storage_error)?,
                row.try_get("encryption_key_id").map_err(map_storage_error)?,
                row.try_get("fingerprint").map_err(map_storage_error)?,
            )
            .map_err(admin_error)?;
            let record = AdminCredentialRuntimeRecord {
                id: row.try_get("id").map_err(map_storage_error)?,
                kind,
                scope: row.try_get("scope").map_err(map_storage_error)?,
                envelope,
                created_at: row.try_get("created_at").map_err(map_storage_error)?,
            };
            record
                .validate()
                .then_some(record)
                .ok_or_else(|| admin_error(AdminCredentialError::StoredResultCorrupt))
        })
        .collect()
}

#[cfg(feature = "postgres")]
pub(super) async fn mark_invalid_postgres(
    store: &super::super::PostgresStore,
    credential_id: Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), ApplicationError> {
    let affected = sqlx::query(
        "UPDATE public.credential_keys
         SET status = 'invalid', state_version = state_version + 1, updated_at = $2
         WHERE id = $1 AND status <> 'invalid'",
    )
    .bind(credential_id)
    .bind(now)
    .execute(store.pool())
    .await
    .map_err(map_storage_error)?
    .rows_affected();
    if affected == 1 {
        return Ok(());
    }
    let status = sqlx::query_scalar::<_, Option<String>>(
        "SELECT status FROM public.credential_keys WHERE id = $1",
    )
    .bind(credential_id)
    .fetch_optional(store.pool())
    .await
    .map_err(map_storage_error)?
    .flatten();
    match status.as_deref() {
        Some("invalid") => Ok(()),
        Some(_) => Err(storage_error("确认凭据失效状态失败")),
        None => Err(admin_error(AdminCredentialError::NotFound)),
    }
}

#[cfg(feature = "postgres")]
pub(super) async fn health_postgres(
    store: &super::super::PostgresStore,
) -> Result<AdminCredentialHealthSnapshot, ApplicationError> {
    let last_admin_change_at = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
        "SELECT MAX(created_at) FROM public.credential_admin_commands",
    )
    .fetch_one(store.pool())
    .await
    .map_err(map_storage_error)?;
    let rows = sqlx::query(
        "SELECT credential_kind, scope, status, count(*)::bigint AS count,
                MAX(updated_at)::timestamptz AS updated_at
         FROM public.credential_keys
         WHERE credential_kind IN ('mail_cookie', 'forum_cookie')
         GROUP BY credential_kind, scope, status
         ORDER BY credential_kind, scope, status",
    )
    .fetch_all(store.pool())
    .await
    .map_err(map_storage_error)?;
    let mut scopes = Vec::new();
    for row in rows {
        let kind = AdminCredentialKind::parse(
            &row.try_get::<String, _>("credential_kind").map_err(map_storage_error)?,
        )
        .ok_or_else(|| admin_error(AdminCredentialError::StoredResultCorrupt))?;
        let scope = row.try_get::<String, _>("scope").map_err(map_storage_error)?;
        let world = world_from_scope(kind, &scope)
            .ok_or_else(|| admin_error(AdminCredentialError::StoredResultCorrupt))?;
        let status = AdminCredentialStatus::parse(
            &row.try_get::<String, _>("status").map_err(map_storage_error)?,
        )
        .ok_or_else(|| admin_error(AdminCredentialError::StoredResultCorrupt))?;
        let count = row.try_get::<i64, _>("count").map_err(map_storage_error)?;
        let updated_at =
            row.try_get::<Option<DateTime<Utc>>, _>("updated_at").map_err(map_storage_error)?;
        if count < 0 {
            return Err(admin_error(AdminCredentialError::StoredResultCorrupt));
        }
        let Some(scope_row) = scopes.iter_mut().find(|item: &&mut AdminCredentialHealthScope| {
            item.kind == kind && item.world == world
        }) else {
            scopes.push(AdminCredentialHealthScope {
                kind,
                world,
                active_count: 0,
                disabled_count: 0,
                invalid_count: 0,
                updated_at,
            });
            let scope_row = scopes
                .last_mut()
                .ok_or_else(|| admin_error(AdminCredentialError::StoredResultCorrupt))?;
            add_health_count(scope_row, status, count)?;
            continue;
        };
        if scope_row.updated_at != updated_at {
            scope_row.updated_at = match (scope_row.updated_at, updated_at) {
                (Some(previous), Some(current)) => Some(previous.max(current)),
                (None, current) => current,
                (current, None) => current,
            };
        }
        add_health_count(scope_row, status, count)?;
    }
    let snapshot = AdminCredentialHealthSnapshot {
        observed_at: chrono::Utc::now(),
        last_admin_change_at,
        scopes,
    };
    snapshot
        .validate()
        .then_some(snapshot)
        .ok_or_else(|| admin_error(AdminCredentialError::StoredResultCorrupt))
}

#[cfg(feature = "postgres")]
pub(super) fn add_health_count(
    scope: &mut AdminCredentialHealthScope,
    status: AdminCredentialStatus,
    count: i64,
) -> Result<(), ApplicationError> {
    let target = match status {
        AdminCredentialStatus::Active => &mut scope.active_count,
        AdminCredentialStatus::Disabled => &mut scope.disabled_count,
        AdminCredentialStatus::Invalid => &mut scope.invalid_count,
    };
    *target = target
        .checked_add(count)
        .ok_or_else(|| admin_error(AdminCredentialError::StoredResultCorrupt))?;
    Ok(())
}
