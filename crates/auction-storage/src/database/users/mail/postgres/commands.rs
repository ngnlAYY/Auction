//! PostgreSQL Users/MoogleMail 注册、找回和换绑命令。

use super::mapping::*;
use super::reads::load_mail_command_tx;
use super::*;

pub(crate) async fn register_postgres(
    store: &crate::database::PostgresStore,
    write: MailUserRegisterWrite,
) -> Result<MailUserResult, ApplicationError> {
    validate_register_write(&write)?;
    let mut transaction = store.pool().begin().await.map_err(map_storage_error)?;
    set_serializable(&mut transaction).await?;
    lock_command(&mut transaction, write.command.command_id).await?;

    if let Some(result) = load_mail_command_tx(
        &mut transaction,
        write.command.command_id,
        MailUserCommandKind::Register,
        write.command.request_hash,
    )
    .await?
    {
        transaction.commit().await.map_err(map_storage_error)?;
        return Ok(result);
    }

    let rows = sqlx::query(
        "SELECT id, username, status, key_status, game_player_id,
                (lower(username) = lower($1)) AS username_matches,
                (game_player_id IS NOT NULL AND lower(game_player_id) = lower($2))
                    AS game_player_matches
         FROM public.users
         WHERE lower(username) = lower($1)
            OR lower(game_player_id) = lower($2)
         ORDER BY id
         FOR UPDATE",
    )
    .bind(&write.command.username)
    .bind(&write.command.game_player_id)
    .fetch_all(&mut *transaction)
    .await
    .map_err(map_storage_error)?;

    let result = match rows.as_slice() {
        [] => {
            sqlx::query(
                "INSERT INTO public.users
                    (id, username, status, public_key_hash, game_player_id,
                     key_status, state_version, created_at, updated_at)
                 VALUES ($1, $2, 'active', $3, $4, 'active', 1, $5, $5)",
            )
            .bind(write.user_id)
            .bind(&write.command.username)
            .bind(write.public_key_hash.to_vec())
            .bind(&write.command.game_player_id)
            .bind(write.command.now)
            .execute(&mut *transaction)
            .await
            .map_err(map_storage_error)?;
            insert_login_key_envelope(
                &mut transaction,
                write.user_id,
                &write.envelope,
                write.command.now,
            )
            .await?;
            MailUserResult { user_id: Some(write.user_id), code: MailUserResultCode::Registered }
        }
        [row] => {
            let user = registration_user(row)?;
            if user.username_matches && user.game_player_matches
                || user.game_player_matches
                    && forum_only_identity(&mut transaction, user.id).await?
            {
                if user.status == "blacklisted" {
                    MailUserResult { user_id: Some(user.id), code: MailUserResultCode::Blacklisted }
                } else {
                    let envelope_exists: bool = sqlx::query_scalar(
                        "SELECT EXISTS (
                             SELECT 1 FROM public.user_login_key_envelopes WHERE user_id = $1
                         )",
                    )
                    .bind(user.id)
                    .fetch_one(&mut *transaction)
                    .await
                    .map_err(map_storage_error)?;
                    let reissue = user.key_status != "active" || !envelope_exists;
                    let identity_changed = user.username != write.command.username
                        || user.game_player_id.as_deref()
                            != Some(write.command.game_player_id.as_str());
                    if reissue || identity_changed {
                        sqlx::query(
                            "UPDATE public.users
                             SET username = $1,
                                 game_player_id = $2,
                                 public_key_hash = CASE WHEN $3 THEN $4 ELSE public_key_hash END,
                                 key_status = CASE WHEN $3 THEN 'active' ELSE key_status END,
                                 state_version = state_version + 1,
                                 updated_at = $5
                             WHERE id = $6",
                        )
                        .bind(&write.command.username)
                        .bind(&write.command.game_player_id)
                        .bind(reissue)
                        .bind(write.public_key_hash.to_vec())
                        .bind(write.command.now)
                        .bind(user.id)
                        .execute(&mut *transaction)
                        .await
                        .map_err(map_storage_error)?;
                    }
                    if reissue {
                        insert_login_key_envelope(
                            &mut transaction,
                            user.id,
                            &write.envelope,
                            write.command.now,
                        )
                        .await?;
                    }
                    MailUserResult { user_id: Some(user.id), code: MailUserResultCode::Recovered }
                }
            } else {
                MailUserResult { user_id: None, code: MailUserResultCode::IdentityConflict }
            }
        }
        _ => MailUserResult { user_id: None, code: MailUserResultCode::IdentityConflict },
    };
    persist_user_mail_result(
        &mut transaction,
        write.command.command_id,
        MailUserCommandKind::Register,
        write.command.request_hash,
        result,
        write.command.now,
        "mail_user_register",
    )
    .await?;
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(result)
}

pub(crate) async fn rebind_postgres(
    store: &crate::database::PostgresStore,
    command: MailUserRebindCommand,
) -> Result<MailUserResult, ApplicationError> {
    validate_rebind_command(&command)?;
    let mut transaction = store.pool().begin().await.map_err(map_storage_error)?;
    set_serializable(&mut transaction).await?;
    lock_command(&mut transaction, command.command_id).await?;
    if let Some(result) = load_mail_command_tx(
        &mut transaction,
        command.command_id,
        MailUserCommandKind::Rebind,
        command.request_hash,
    )
    .await?
    {
        transaction.commit().await.map_err(map_storage_error)?;
        return Ok(result);
    }

    let rows = sqlx::query(
        "SELECT id, username, status, public_key_hash, game_player_id
         FROM public.users
         WHERE public_key_hash = $1
            OR lower(username) = lower($2)
            OR lower(game_player_id) = lower($3)
         ORDER BY id
         FOR UPDATE",
    )
    .bind(command.old_key_hash.to_vec())
    .bind(&command.next_username)
    .bind(&command.next_game_player_id)
    .fetch_all(&mut *transaction)
    .await
    .map_err(map_storage_error)?;

    let source = rows.iter().find(|row| {
        row.try_get::<Vec<u8>, _>("public_key_hash")
            .ok()
            .is_some_and(|hash| hash.as_slice().ct_eq(&command.old_key_hash).into())
    });
    let result = match source {
        None => MailUserResult { user_id: None, code: MailUserResultCode::InvalidKey },
        Some(source) => {
            let user_id: Uuid = source.try_get("id").map_err(map_storage_error)?;
            let status: String = source.try_get("status").map_err(map_storage_error)?;
            if status == "blacklisted" {
                MailUserResult { user_id: Some(user_id), code: MailUserResultCode::Blacklisted }
            } else {
                let conflict = rows.iter().any(|row| {
                    let Ok(other_id) = row.try_get::<Uuid, _>("id") else {
                        return true;
                    };
                    if other_id == user_id {
                        return false;
                    }
                    let username_matches = row
                        .try_get::<String, _>("username")
                        .is_ok_and(|value| value.eq_ignore_ascii_case(&command.next_username));
                    let player_matches = row
                        .try_get::<Option<String>, _>("game_player_id")
                        .ok()
                        .flatten()
                        .is_some_and(|value| {
                            value.eq_ignore_ascii_case(&command.next_game_player_id)
                        });
                    username_matches || player_matches
                });
                if conflict {
                    MailUserResult {
                        user_id: Some(user_id),
                        code: MailUserResultCode::IdentityConflict,
                    }
                } else {
                    let username: String = source.try_get("username").map_err(map_storage_error)?;
                    let game_player_id: Option<String> =
                        source.try_get("game_player_id").map_err(map_storage_error)?;
                    let unchanged = username == command.next_username
                        && game_player_id.as_deref() == Some(command.next_game_player_id.as_str());
                    if unchanged {
                        MailUserResult {
                            user_id: Some(user_id),
                            code: MailUserResultCode::Unchanged,
                        }
                    } else {
                        let affected = sqlx::query(
                            "UPDATE public.users
                             SET username = $1,
                                 game_player_id = $2,
                                 state_version = state_version + 1,
                                 updated_at = $3
                             WHERE id = $4",
                        )
                        .bind(&command.next_username)
                        .bind(&command.next_game_player_id)
                        .bind(command.now)
                        .bind(user_id)
                        .execute(&mut *transaction)
                        .await
                        .map_err(map_storage_error)?;
                        if affected.rows_affected() != 1 {
                            return Err(repository_error("换绑用户更新结果异常"));
                        }
                        sqlx::query(
                            "INSERT INTO public.user_game_identity_history
                                (id, user_id, source_command_id,
                                 previous_username, next_username,
                                 previous_game_player_id, next_game_player_id, created_at)
                             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                        )
                        .bind(command.history_id)
                        .bind(user_id)
                        .bind(command.command_id.as_uuid())
                        .bind(username)
                        .bind(&command.next_username)
                        .bind(game_player_id)
                        .bind(&command.next_game_player_id)
                        .bind(command.now)
                        .execute(&mut *transaction)
                        .await
                        .map_err(map_storage_error)?;
                        MailUserResult { user_id: Some(user_id), code: MailUserResultCode::Rebound }
                    }
                }
            }
        }
    };
    persist_user_mail_result(
        &mut transaction,
        command.command_id,
        MailUserCommandKind::Rebind,
        command.request_hash,
        result,
        command.now,
        "mail_user_rebind",
    )
    .await?;
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(result)
}
