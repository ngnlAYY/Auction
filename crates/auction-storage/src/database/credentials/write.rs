//! Admin 凭据导入、修改、删除及其事务流程。

use super::mapping::*;

use super::*;

#[cfg(feature = "postgres")]
pub(super) async fn import_postgres(
    store: &super::super::PostgresStore,
    write: AdminCredentialImportWrite,
) -> Result<AdminCredentialCommandResult, ApplicationError> {
    validate_import_write(&write)?;
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启凭据导入事务失败"))?;
    set_serializable(&mut transaction).await?;
    lock_command(&mut transaction, write.command_id).await?;
    if let Some(result) = find_command_in_transaction(
        &mut transaction,
        write.command_id,
        AdminCredentialCommandKind::Import,
        write.request_hash,
    )
    .await?
    {
        transaction.commit().await.map_err(|_| storage_error("提交凭据导入重放事务失败"))?;
        return Ok(result);
    }

    let scope = write.world.scope(write.kind);
    sqlx::query(
        "INSERT INTO public.credential_keys
            (id, credential_kind, scope, label, ciphertext, nonce, encryption_key_id,
             fingerprint, status, state_version, created_at, updated_at, player_name)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'active', 1, $9, $9, $10)",
    )
    .bind(write.credential_id)
    .bind(write.kind.as_str())
    .bind(&scope)
    .bind(&write.label)
    .bind(write.envelope.ciphertext())
    .bind(write.envelope.nonce())
    .bind(write.envelope.encryption_key_id())
    .bind(write.envelope.fingerprint())
    .bind(write.now)
    .bind(&write.player_name)
    .execute(&mut *transaction)
    .await
    .map_err(map_write_error)?;
    let credential = AdminCredential {
        id: write.credential_id,
        kind: write.kind,
        world: write.world,
        label: write.label.clone(),
        player_name: write.player_name.clone(),
        status: AdminCredentialStatus::Active,
        state_version: 1,
        created_at: write.now,
        rotated_at: None,
        updated_at: write.now,
    };
    let result = credential_result(credential)?;
    insert_command(
        &mut transaction,
        write.command_id,
        AdminCredentialCommandKind::Import,
        write.request_hash,
        write.credential_id,
        &result,
        write.now,
    )
    .await?;
    insert_audit(
        &mut transaction,
        write.command_id,
        AdminCredentialCommandKind::Import,
        write.credential_id,
        &write.actor_id,
        write.request_id.as_deref(),
        write.now,
    )
    .await?;
    transaction.commit().await.map_err(|_| storage_error("提交凭据导入事务失败"))?;
    Ok(result)
}

#[cfg(feature = "postgres")]
pub(super) async fn mutate_postgres(
    store: &super::super::PostgresStore,
    write: AdminCredentialMutationWrite,
) -> Result<AdminCredentialCommandResult, ApplicationError> {
    validate_mutation_write(&write)?;
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启凭据修改事务失败"))?;
    set_serializable(&mut transaction).await?;
    lock_command(&mut transaction, write.command_id).await?;
    if let Some(result) = find_command_in_transaction(
        &mut transaction,
        write.command_id,
        write.action,
        write.request_hash,
    )
    .await?
    {
        transaction.commit().await.map_err(|_| storage_error("提交凭据修改重放事务失败"))?;
        return Ok(result);
    }
    let before = load_credential_for_update(&mut transaction, write.credential_id).await?;
    if before.state_version != write.expected_state_version {
        return Err(admin_error(AdminCredentialError::VersionConflict {
            current_version: before.state_version,
        }));
    }
    let next_version = write
        .expected_state_version
        .checked_add(1)
        .ok_or_else(|| admin_error(AdminCredentialError::InvalidRequest))?;
    let next_label = write.label.as_deref().unwrap_or(&before.label);
    let next_status = write.status.unwrap_or(before.status);
    if write.action == AdminCredentialCommandKind::Rename && write.label.is_none()
        || write.action == AdminCredentialCommandKind::SetStatus && write.status.is_none()
    {
        return Err(admin_error(AdminCredentialError::InvalidRequest));
    }
    // 与用户快照一致，凭据修改不能倒退时间；首次停用也必须满足 rotated_at 的创建时间下界。
    let row = sqlx::query(
        "UPDATE public.credential_keys
         SET label = $2, status = $3, state_version = $4,
             updated_at = GREATEST(created_at, updated_at, $5),
             rotated_at = CASE WHEN $3 = 'active' THEN rotated_at
                 ELSE COALESCE(rotated_at, GREATEST(created_at, updated_at, $5)) END
         WHERE id = $1 AND state_version = $6
         RETURNING id, credential_kind, scope, label, player_name, status, state_version,
                   created_at, rotated_at, updated_at",
    )
    .bind(write.credential_id)
    .bind(next_label)
    .bind(next_status.as_str())
    .bind(next_version)
    .bind(write.now)
    .bind(write.expected_state_version)
    .fetch_one(&mut *transaction)
    .await
    .map_err(map_write_error)?;
    let credential = map_postgres_credential(&row)?;
    let result = credential_result(credential)?;
    insert_command(
        &mut transaction,
        write.command_id,
        write.action,
        write.request_hash,
        write.credential_id,
        &result,
        write.now,
    )
    .await?;
    insert_audit(
        &mut transaction,
        write.command_id,
        write.action,
        write.credential_id,
        &write.actor_id,
        write.request_id.as_deref(),
        write.now,
    )
    .await?;
    transaction.commit().await.map_err(|_| storage_error("提交凭据修改事务失败"))?;
    Ok(result)
}

#[cfg(feature = "postgres")]
pub(super) async fn delete_postgres(
    store: &super::super::PostgresStore,
    write: AdminCredentialDeleteWrite,
) -> Result<AdminCredentialCommandResult, ApplicationError> {
    validate_delete_write(&write)?;
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启凭据删除事务失败"))?;
    set_serializable(&mut transaction).await?;
    lock_command(&mut transaction, write.command_id).await?;
    if let Some(result) = find_command_in_transaction(
        &mut transaction,
        write.command_id,
        AdminCredentialCommandKind::Delete,
        write.request_hash,
    )
    .await?
    {
        transaction.commit().await.map_err(|_| storage_error("提交凭据删除重放事务失败"))?;
        return Ok(result);
    }
    let before = load_credential_for_update(&mut transaction, write.credential_id).await?;
    if before.state_version != write.expected_state_version {
        return Err(admin_error(AdminCredentialError::VersionConflict {
            current_version: before.state_version,
        }));
    }
    let deleted = AdminCredentialDeleteResult {
        credential_id: write.credential_id,
        state_version: write
            .expected_state_version
            .checked_add(1)
            .ok_or_else(|| admin_error(AdminCredentialError::InvalidRequest))?,
    };
    let result = deleted_result(deleted)?;
    insert_command(
        &mut transaction,
        write.command_id,
        AdminCredentialCommandKind::Delete,
        write.request_hash,
        write.credential_id,
        &result,
        write.now,
    )
    .await?;
    insert_audit(
        &mut transaction,
        write.command_id,
        AdminCredentialCommandKind::Delete,
        write.credential_id,
        &write.actor_id,
        write.request_id.as_deref(),
        write.now,
    )
    .await?;
    sqlx::query("DELETE FROM public.credential_keys WHERE id = $1 AND state_version = $2")
        .bind(write.credential_id)
        .bind(write.expected_state_version)
        .execute(&mut *transaction)
        .await
        .map_err(map_storage_error)?;
    transaction.commit().await.map_err(|_| storage_error("提交凭据删除事务失败"))?;
    Ok(result)
}

#[cfg(feature = "postgres")]
pub(super) async fn find_command_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command_id: Uuid,
    kind: AdminCredentialCommandKind,
    request_hash: RequestHash,
) -> Result<Option<AdminCredentialCommandResult>, ApplicationError> {
    let row = sqlx::query(
        "SELECT command_kind, request_digest, credential_id, result_payload
         FROM public.credential_admin_commands
         WHERE command_id = $1
         FOR UPDATE",
    )
    .bind(command_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    row.map(|row| parse_command(&row, kind, request_hash)).transpose()
}

#[cfg(feature = "postgres")]
pub(super) async fn load_credential_for_update(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    credential_id: Uuid,
) -> Result<AdminCredential, ApplicationError> {
    let row = sqlx::query(
        "SELECT id, credential_kind, scope, label, player_name, status, state_version,
                created_at, rotated_at, updated_at
         FROM public.credential_keys
         WHERE id = $1
         FOR UPDATE",
    )
    .bind(credential_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_storage_error)?
    .ok_or_else(|| admin_error(AdminCredentialError::NotFound))?;
    map_postgres_credential(&row)
}
