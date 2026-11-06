//! PostgreSQL Users/MoogleMail 注册、找回和换绑命令。
//!
//! 用户与身份快照的更新时间只向前推进；兼容旧外部时间高水位，但不改写命令账本时间。

use super::mapping::*;
use super::reads::load_mail_command_tx;
use crate::database::users::envelope::upsert_login_key_envelope;
use crate::database::users::mail::repository_error;
use auction_application::{
    ApplicationError, MailUserCommandKind, MailUserRebindCommand, MailUserRegisterWrite,
    MailUserRekeyWrite, MailUserResult, MailUserResultCode,
};
use sqlx::Row;
use subtle::ConstantTimeEq;
use uuid::Uuid;

pub(crate) async fn register_postgres(
    store: &crate::database::PostgresStore,
    write: MailUserRegisterWrite,
) -> Result<MailUserResult, ApplicationError> {
    store
        .pool()
        .observe_transaction(
            auction_observability::DatabaseTransactionOwner::PublicIdentity,
            async {
                validate_register_write(&write)?;
                let mut transaction = store
                    .pool()
                    .begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE")
                    .await
                    .map_err(map_storage_error)?;
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
                    "SELECT id, username, status, key_status,
                (lower(username) = lower($1)) AS username_matches
         FROM public.users
         WHERE lower(username) = lower($1)
         ORDER BY id
         FOR UPDATE",
                )
                .bind(&write.command.username)
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
                        .bind(write.public_key_hash.as_slice())
                        .bind(&write.command.username)
                        .bind(write.command.now)
                        .execute(&mut *transaction)
                        .await
                        .map_err(map_storage_error)?;
                        upsert_login_key_envelope(
                            &mut transaction,
                            write.user_id,
                            &write.envelope,
                            write.command.now,
                        )
                        .await
                        .map_err(map_storage_error)?;
                        MailUserResult {
                            user_id: Some(write.user_id),
                            code: MailUserResultCode::Registered,
                        }
                    }
                    [row] => {
                        let user = registration_user(row)?;
                        // 身份预读与写锁之间可能发生注册或改名；不允许把别的 UUID 的 AAD 信封写入此用户。
                        // Repository 错误由现有邮件重试重新执行 lookup 与签发，此处不写入结果账本。
                        if user.id != write.user_id {
                            return Err(repository_error(
                                "Users MoogleMail 注册身份已变化，必须重新读取后重试",
                            ));
                        }
                        if user.username_matches {
                            if user.status == "blacklisted" {
                                MailUserResult {
                                    user_id: Some(user.id),
                                    code: MailUserResultCode::Blacklisted,
                                }
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
                                let identity_changed = user.username != write.command.username;
                                if identity_changed {
                                    // 大小写修正也是地址变更；与换绑一样保留原名称，供旧稳定退款摘要精确重建。
                                    sqlx::query(
                                        "INSERT INTO public.user_game_identity_history
                                (id, user_id, source_command_id,
                                 previous_username, next_username,
                                 previous_game_player_id, next_game_player_id, created_at)
                             VALUES ($1, $2, $1, $3, $4, $3, $4, $5)",
                                    )
                                    .bind(write.command.command_id.as_uuid())
                                    .bind(user.id)
                                    .bind(&user.username)
                                    .bind(&write.command.username)
                                    .bind(write.command.now)
                                    .execute(&mut *transaction)
                                    .await
                                    .map_err(map_storage_error)?;
                                }
                                if reissue || identity_changed {
                                    sqlx::query(
                                        "UPDATE public.users
                             SET username = $1,
                                 game_player_id = $1,
                                 public_key_hash = CASE WHEN $2 THEN $3 ELSE public_key_hash END,
                                 key_status = CASE WHEN $2 THEN 'active' ELSE key_status END,
                                 state_version = state_version + 1,
                                 updated_at = GREATEST(created_at, updated_at, $4)
                             WHERE id = $5",
                                    )
                                    .bind(&write.command.username)
                                    .bind(reissue)
                                    .bind(write.public_key_hash.as_slice())
                                    .bind(write.command.now)
                                    .bind(user.id)
                                    .execute(&mut *transaction)
                                    .await
                                    .map_err(map_storage_error)?;
                                }
                                if reissue {
                                    // 邮件找回触发的重签与 Admin 重签采用相同规则，旧 token 不随新 Key 恢复。
                                    sqlx::query(
                                        "UPDATE public.public_sessions
                             SET revoked_at = GREATEST(created_at, $1)
                             WHERE user_id = $2 AND revoked_at IS NULL",
                                    )
                                    .bind(write.command.now)
                                    .bind(user.id)
                                    .execute(&mut *transaction)
                                    .await
                                    .map_err(map_storage_error)?;
                                    upsert_login_key_envelope(
                                        &mut transaction,
                                        user.id,
                                        &write.envelope,
                                        write.command.now,
                                    )
                                    .await
                                    .map_err(map_storage_error)?;
                                }
                                if forum_only_identity(&mut transaction, user.id).await? {
                                    sqlx::query(
                                        "UPDATE public.forum_bidder_identities
                             SET registration_state = 'mm_bound',
                                 updated_at = GREATEST(created_at, updated_at, $2)
                             WHERE user_id = $1 AND registration_state = 'forum_only'",
                                    )
                                    .bind(user.id)
                                    .bind(write.command.now)
                                    .execute(&mut *transaction)
                                    .await
                                    .map_err(map_storage_error)?;
                                }
                                MailUserResult {
                                    user_id: Some(user.id),
                                    code: MailUserResultCode::Recovered,
                                }
                            }
                        } else {
                            MailUserResult {
                                user_id: None,
                                code: MailUserResultCode::IdentityConflict,
                            }
                        }
                    }
                    _ => {
                        MailUserResult { user_id: None, code: MailUserResultCode::IdentityConflict }
                    }
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
            },
        )
        .await
}

pub(crate) async fn rebind_postgres(
    store: &crate::database::PostgresStore,
    command: MailUserRebindCommand,
) -> Result<MailUserResult, ApplicationError> {
    store
        .pool()
        .observe_transaction(
            auction_observability::DatabaseTransactionOwner::PublicIdentity,
            async {
                validate_rebind_command(&command)?;
                let mut transaction = store
                    .pool()
                    .begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE")
                    .await
                    .map_err(map_storage_error)?;
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
                    "SELECT id, username, status, key_status, public_key_hash
         FROM public.users
         WHERE public_key_hash = $1
            OR lower(username) = lower($2)
         ORDER BY id
         FOR UPDATE",
                )
                .bind(command.old_key_hash.as_slice())
                .bind(&command.next_username)
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
                        let key_status: String =
                            source.try_get("key_status").map_err(map_storage_error)?;
                        if status == "blacklisted" {
                            MailUserResult {
                                user_id: Some(user_id),
                                code: MailUserResultCode::Blacklisted,
                            }
                        } else if key_status != "active" {
                            // 撤销的 key 不再授权新换绑；冲突目标仍保留在完整查询中，不能复用其名称。
                            MailUserResult { user_id: None, code: MailUserResultCode::InvalidKey }
                        } else {
                            let conflict = rows.iter().any(|row| {
                                let Ok(other_id) = row.try_get::<Uuid, _>("id") else {
                                    return true;
                                };
                                if other_id == user_id {
                                    return false;
                                }
                                row.try_get::<String, _>("username").is_ok_and(|value| {
                                    value.eq_ignore_ascii_case(&command.next_username)
                                })
                            });
                            if conflict {
                                MailUserResult {
                                    user_id: Some(user_id),
                                    code: MailUserResultCode::IdentityConflict,
                                }
                            } else {
                                let username: String =
                                    source.try_get("username").map_err(map_storage_error)?;
                                let unchanged = username == command.next_username;
                                if unchanged {
                                    MailUserResult {
                                        user_id: Some(user_id),
                                        code: MailUserResultCode::Unchanged,
                                    }
                                } else {
                                    let affected = sqlx::query(
                                        "UPDATE public.users
                             SET username = $1,
                                 game_player_id = $1,
                                 state_version = state_version + 1,
                                 updated_at = GREATEST(created_at, updated_at, $2)
                             WHERE id = $3",
                                    )
                                    .bind(&command.next_username)
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
                                    .bind(
                                        source
                                            .try_get::<String, _>("username")
                                            .map_err(map_storage_error)?,
                                    )
                                    .bind(&command.next_username)
                                    .bind(command.now)
                                    .execute(&mut *transaction)
                                    .await
                                    .map_err(map_storage_error)?;
                                    MailUserResult {
                                        user_id: Some(user_id),
                                        code: MailUserResultCode::Rebound,
                                    }
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
            },
        )
        .await
}

/// 自助轮换登录 key：新摘要、加密信封、全部会话注销与命令账本必须在同一 Serializable
/// 事务内提交。被拉黑或身份冲突时不改写 key；账号不存在只记录 not_registered。
pub(crate) async fn rekey_postgres(
    store: &crate::database::PostgresStore,
    write: MailUserRekeyWrite,
) -> Result<MailUserResult, ApplicationError> {
    store
        .pool()
        .observe_transaction(
            auction_observability::DatabaseTransactionOwner::PublicIdentity,
            async {
                validate_rekey_write(&write)?;
                let mut transaction = store
                    .pool()
                    .begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE")
                    .await
                    .map_err(map_storage_error)?;
                lock_command(&mut transaction, write.command.command_id).await?;

                if let Some(result) = load_mail_command_tx(
                    &mut transaction,
                    write.command.command_id,
                    MailUserCommandKind::Rekey,
                    write.command.request_hash,
                )
                .await?
                {
                    transaction.commit().await.map_err(map_storage_error)?;
                    return Ok(result);
                }

                let rows = sqlx::query(
                    "SELECT id, username, status
         FROM public.users
         WHERE lower(username) = lower($1)
         ORDER BY id
         FOR UPDATE",
                )
                .bind(&write.command.username)
                .fetch_all(&mut *transaction)
                .await
                .map_err(map_storage_error)?;

                let result = match rows.as_slice() {
                    [] => MailUserResult { user_id: None, code: MailUserResultCode::NotRegistered },
                    [row] => {
                        let user_id: Uuid = row.try_get("id").map_err(map_storage_error)?;
                        let username: String =
                            row.try_get("username").map_err(map_storage_error)?;
                        let status: String = row.try_get("status").map_err(map_storage_error)?;
                        // 预读身份与写锁之间可能发生注册或改名；信封 AAD 不能写入别的 UUID。
                        if user_id != write.user_id {
                            return Err(repository_error(
                                "Users MoogleMail 轮换身份已变化，必须重新读取后重试",
                            ));
                        }
                        if status == "blacklisted" {
                            MailUserResult {
                                user_id: Some(user_id),
                                code: MailUserResultCode::Blacklisted,
                            }
                        } else if username != write.command.username {
                            // 与 register 一致：大小写重命名属于另一个身份请求，不静默改写 key。
                            MailUserResult {
                                user_id: None,
                                code: MailUserResultCode::IdentityConflict,
                            }
                        } else {
                            let affected = sqlx::query(
                                "UPDATE public.users
                         SET public_key_hash = $1,
                             key_status = 'active',
                             state_version = state_version + 1,
                             updated_at = GREATEST(created_at, updated_at, $2)
                         WHERE id = $3",
                            )
                            .bind(write.public_key_hash.as_slice())
                            .bind(write.command.now)
                            .bind(user_id)
                            .execute(&mut *transaction)
                            .await
                            .map_err(map_storage_error)?;
                            if affected.rows_affected() != 1 {
                                return Err(repository_error("轮换用户更新结果异常"));
                            }
                            // 旧 key 泄露后旧会话不能再继续使用；保留行只标记注销。
                            sqlx::query(
                                "UPDATE public.public_sessions
                         SET revoked_at = GREATEST(created_at, $1)
                         WHERE user_id = $2 AND revoked_at IS NULL",
                            )
                            .bind(write.command.now)
                            .bind(user_id)
                            .execute(&mut *transaction)
                            .await
                            .map_err(map_storage_error)?;
                            upsert_login_key_envelope(
                                &mut transaction,
                                user_id,
                                &write.envelope,
                                write.command.now,
                            )
                            .await
                            .map_err(map_storage_error)?;
                            MailUserResult {
                                user_id: Some(user_id),
                                code: MailUserResultCode::Rekeyed,
                            }
                        }
                    }
                    _ => {
                        MailUserResult { user_id: None, code: MailUserResultCode::IdentityConflict }
                    }
                };
                persist_user_mail_result(
                    &mut transaction,
                    write.command.command_id,
                    MailUserCommandKind::Rekey,
                    write.command.request_hash,
                    result,
                    write.command.now,
                    "mail_user_rekey",
                )
                .await?;
                transaction.commit().await.map_err(map_storage_error)?;
                Ok(result)
            },
        )
        .await
}
