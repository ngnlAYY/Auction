//! Public 登录偏好事务：先锁用户、再锁当前会话，授权复核先于命令重放。

use auction_application::core::{
    ApplicationError, CommandLedgerError, CompetitionTransactionError, RequestHash, SessionError,
    StoredCommand, TransactionPolicy, TransactionRunner,
};
use auction_application::public::{
    PublicSessionIdentity, PublicSessionPreference, PublicSessionPreferenceResult,
    SetPublicSessionPreferenceCommand,
};
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use super::{map_identity, map_preference, storage_error};
use crate::database::{
    DatabaseStore, PostgresCompetitionTransaction, audit_health::AuditWriteResult as _,
    competition::map_sql_error,
};

pub(in crate::database::sessions) async fn get_postgres(
    store: &DatabaseStore,
    token_hash: [u8; 32],
    now: DateTime<Utc>,
) -> Result<PublicSessionPreferenceResult, ApplicationError> {
    // 与只读 inspect 共用池等待后的截止门禁，不能按请求开始时间返回过期身份。
    let row = sqlx::query(
        "SELECT session.user_id, account.username,
                account.public_session_duration_seconds, account.public_session_preference_version,
                session.expires_at, session.absolute_expires_at
         FROM public.public_sessions AS session
         JOIN public.users AS account ON account.id = session.user_id
         WHERE session.token_hash = $1 AND session.revoked_at IS NULL
           AND ((session.expires_at IS NULL AND session.absolute_expires_at IS NULL)
             OR (session.expires_at > GREATEST($2, clock_timestamp())
               AND session.absolute_expires_at > GREATEST($2, clock_timestamp())))
           AND account.status = 'active' AND account.key_status = 'active'",
    )
    .bind(token_hash.as_slice())
    .bind(now)
    .fetch_optional(store.pool())
    .await
    .map_err(map_sql_error)?
    .ok_or(SessionError::Unauthenticated)?;
    let result = PublicSessionPreferenceResult {
        preference: map_preference(&row)?,
        identity: map_identity(&row)?,
    };
    result.validate().map_err(|_| storage_error("Public 登录时长偏好投影无效"))?;
    Ok(result)
}

pub(in crate::database::sessions) async fn set_postgres(
    store: &DatabaseStore,
    token_hash: [u8; 32],
    command: SetPublicSessionPreferenceCommand,
    now: DateTime<Utc>,
) -> Result<PublicSessionPreferenceResult, ApplicationError> {
    let request_hash = command.request_hash(token_hash)?;
    store
        .for_transaction(auction_observability::DatabaseTransactionOwner::PublicIdentity)
        .run(TransactionPolicy::serializable(), |transaction| {
            Box::pin(set_locked(transaction, token_hash, command, request_hash, now))
        })
        .await
}

async fn set_locked(
    transaction: &mut PostgresCompetitionTransaction,
    token_hash: [u8; 32],
    command: SetPublicSessionPreferenceCommand,
    request_hash: RequestHash,
    now: DateTime<Utc>,
) -> Result<PublicSessionPreferenceResult, ApplicationError> {
    // 与换钥、封禁、并发登录保持 user -> session 顺序；只认仍 active 的账号和 Key。
    let account = sqlx::query(
        "SELECT id AS user_id, username,
                public_session_duration_seconds, public_session_preference_version
         FROM public.users
         WHERE id = $1 AND status = 'active' AND key_status = 'active'
         FOR UPDATE",
    )
    .bind(command.user_id)
    .fetch_optional(&mut **transaction.transaction_mut()?)
    .await
    .map_err(map_sql_error)?
    .ok_or(SessionError::Unauthenticated)?;
    let session = sqlx::query(
        "SELECT user_id, expires_at, absolute_expires_at
         FROM public.public_sessions
         WHERE token_hash = $1 AND user_id = $2 AND revoked_at IS NULL
           AND ((expires_at IS NULL AND absolute_expires_at IS NULL)
             OR (expires_at > $3 AND absolute_expires_at > $3))
         FOR UPDATE",
    )
    .bind(token_hash.as_slice())
    .bind(command.user_id)
    .bind(now)
    .fetch_optional(&mut **transaction.transaction_mut()?)
    .await
    .map_err(map_sql_error)?
    .ok_or(SessionError::Unauthenticated)?;
    let current_identity = PublicSessionIdentity {
        user_id: command.user_id,
        username: account.try_get("username").map_err(map_sql_error)?,
        expires_at: session.try_get("expires_at").map_err(map_sql_error)?,
        absolute_expires_at: session.try_get("absolute_expires_at").map_err(map_sql_error)?,
    };
    let stored = transaction.find_command_inner(command.command_id).await?;
    // 任一行锁等待都可能跨过旧截止；拿到锁后重新取数据库时钟，不能用请求开始时间复活。
    let now = transaction.current_time_inner().await?.max(now);
    if current_identity.effective_expires_at().is_some_and(|expires_at| expires_at <= now) {
        return Err(SessionError::Unauthenticated.into());
    }
    if let Some(stored) = stored {
        if !stored.matches_request(
            SetPublicSessionPreferenceCommand::KIND,
            command.user_id,
            request_hash,
        ) {
            return Err(SessionError::PreferenceConflict.into());
        }
        return replay(stored, command);
    }

    let current = map_preference(&account)?;
    let preference = command.next_preference(current)?;
    let expires_at = preference.expires_at_from(now)?;
    let changed = sqlx::query(
        "UPDATE public.users
         SET public_session_duration_seconds = $2, public_session_preference_version = $3
         WHERE id = $1 AND public_session_preference_version = $4",
    )
    .bind(command.user_id)
    .bind(preference.duration_seconds)
    .bind(preference.version)
    .bind(command.expected_version)
    .execute(&mut **transaction.transaction_mut()?)
    .await
    .map_err(map_sql_error)?
    .rows_affected();
    if changed != 1 {
        return Err(SessionError::PreferenceConflict.into());
    }
    let updated = sqlx::query(
        "UPDATE public.public_sessions
         SET expires_at = $3, absolute_expires_at = $3,
             last_activity_at = GREATEST(last_activity_at, $4)
         WHERE token_hash = $1 AND user_id = $2 AND revoked_at IS NULL
           AND ((expires_at IS NULL AND absolute_expires_at IS NULL)
             OR (expires_at > clock_timestamp() AND absolute_expires_at > clock_timestamp()))
         RETURNING expires_at, absolute_expires_at",
    )
    .bind(token_hash.as_slice())
    .bind(command.user_id)
    .bind(expires_at)
    .bind(now)
    .fetch_optional(&mut **transaction.transaction_mut()?)
    .await
    .map_err(map_sql_error)?
    .ok_or(SessionError::Unauthenticated)?;
    let result = PublicSessionPreferenceResult {
        preference,
        identity: PublicSessionIdentity {
            user_id: command.user_id,
            username: current_identity.username,
            expires_at: updated.try_get("expires_at").map_err(map_sql_error)?,
            absolute_expires_at: updated.try_get("absolute_expires_at").map_err(map_sql_error)?,
        },
    };
    append_audit(
        transaction,
        command,
        current,
        current_identity.expires_at,
        current_identity.absolute_expires_at,
        &result,
        now,
    )
    .await?;
    let stored = StoredCommand {
        command_id: command.command_id,
        command_kind: SetPublicSessionPreferenceCommand::KIND.to_owned(),
        aggregate_id: command.user_id,
        request_hash,
        result_code: "ok".to_owned(),
        result_payload: serde_json::to_vec(&result)
            .map_err(|_| storage_error("编码 Public 登录时长命令结果失败"))?,
    };
    match transaction.insert_command_inner(&stored).await {
        Ok(_) => Ok(result),
        Err(CompetitionTransactionError::CommandLedger(CommandLedgerError::Conflict)) => {
            Err(SessionError::PreferenceConflict.into())
        }
        Err(error) => Err(error.into()),
    }
}

fn replay(
    stored: StoredCommand,
    command: SetPublicSessionPreferenceCommand,
) -> Result<PublicSessionPreferenceResult, ApplicationError> {
    let result: PublicSessionPreferenceResult = serde_json::from_slice(&stored.result_payload)
        .map_err(|_| storage_error("解码 Public 登录时长命令结果失败"))?;
    result.validate().map_err(|_| storage_error("Public 登录时长命令结果无效"))?;
    if stored.result_code != "ok"
        || result.preference.user_id != command.user_id
        || result.preference.duration_seconds != command.duration_seconds
        || Some(result.preference.version) != command.expected_version.checked_add(1)
        || result.identity.expires_at != result.identity.absolute_expires_at
        || result.identity.expires_at.is_some() != command.duration_seconds.is_some()
    {
        return Err(storage_error("Public 登录时长命令结果与请求不一致"));
    }
    // 此处返回原始截止，不再次更新偏好或会话；调用方必须另读当前会话再刷新 Cookie。
    Ok(result)
}

async fn append_audit(
    transaction: &mut PostgresCompetitionTransaction,
    command: SetPublicSessionPreferenceCommand,
    previous: PublicSessionPreference,
    previous_expires_at: Option<DateTime<Utc>>,
    previous_absolute_expires_at: Option<DateTime<Utc>>,
    result: &PublicSessionPreferenceResult,
    now: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO public.audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
             result, source, metadata)
         VALUES ($1, $2, 'public_user', $3, 'session.preference', 'user', $3,
                 'success', 'public', $4)",
    )
    .bind(Uuid::new_v4())
    .bind(now)
    .bind(command.user_id.to_string())
    .bind(serde_json::json!({
        "command_id": command.command_id,
        "previous_duration_seconds": previous.duration_seconds,
        "duration_seconds": result.preference.duration_seconds,
        "previous_version": previous.version,
        "version": result.preference.version,
        "previous_expires_at": previous_expires_at,
        "previous_absolute_expires_at": previous_absolute_expires_at,
        "expires_at": result.identity.expires_at,
        "absolute_expires_at": result.identity.absolute_expires_at,
    }))
    .execute(&mut **transaction.transaction_mut()?)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;
    Ok(())
}
