//! SQLite Lite 用户拉黑的代理冻结与会话撤销副作用。

use super::*;

/// 在 `BEGIN IMMEDIATE` 串行写事务内冻结有效代理并撤销公开会话。
///
/// SQLite 不提供 PostgreSQL 行锁，但仍按 lot、proxy、user 的逻辑顺序读取和写入，保证两种
/// 后端共享同一业务证据形状；调用方在本函数完成后才读取并修改 user。
pub(super) async fn apply_blacklist_effects_sqlite(
    connection: &mut SqliteConnection,
    source_command_id: Uuid,
    user_id: Uuid,
    now: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let user_id_blob = uuid_blob(user_id);
    let lot_rows = sqlx::query(
        "SELECT lot.id
         FROM lots AS lot
         WHERE EXISTS (
             SELECT 1
             FROM bid_proxy_authorizations AS proxy
             WHERE proxy.lot_id = lot.id
               AND proxy.user_id = ?
               AND proxy.status IN ('active', 'final_fast')
         )
         ORDER BY lot.id",
    )
    .bind(&user_id_blob)
    .fetch_all(&mut *connection)
    .await
    .map_err(map_sqlite_error)?;
    let proxies = sqlx::query(
        "SELECT authorization_id, lot_id, maximum_amount, mode, status, state_version
         FROM bid_proxy_authorizations
         WHERE user_id = ? AND status IN ('active', 'final_fast')
         ORDER BY lot_id, user_id, authorization_id",
    )
    .bind(&user_id_blob)
    .fetch_all(&mut *connection)
    .await
    .map_err(map_sqlite_error)?;
    if lot_rows.len() != proxies.len() {
        return Err(admin_error(AdminUserError::StoredResultCorrupt));
    }

    let now = sqlite_timestamp(now);
    for row in proxies {
        let authorization_id =
            uuid_from_blob(row.try_get("authorization_id").map_err(map_sqlite_error)?)?;
        let lot_id = uuid_from_blob(row.try_get("lot_id").map_err(map_sqlite_error)?)?;
        let maximum_amount: i64 = row.try_get("maximum_amount").map_err(map_sqlite_error)?;
        let mode: String = row.try_get("mode").map_err(map_sqlite_error)?;
        let status: String = row.try_get("status").map_err(map_sqlite_error)?;
        let state_version: i64 = row.try_get("state_version").map_err(map_sqlite_error)?;
        let next_version = state_version
            .checked_add(1)
            .filter(|version| maximum_amount > 0 && *version > 1)
            .ok_or_else(|| admin_error(AdminUserError::StoredResultCorrupt))?;
        if !matches!(mode.as_str(), "fast" | "slow")
            || !matches!(status.as_str(), "active" | "final_fast")
        {
            return Err(admin_error(AdminUserError::StoredResultCorrupt));
        }

        let child_command_id = deterministic_uuid(
            CommandId::from_uuid(source_command_id),
            &format!("users-freeze-proxy-authorization:{authorization_id}"),
            1,
        );
        let request = json!({
            "authorization_id": authorization_id,
            "source_command_id": source_command_id,
            "reason": "eligibility_lost",
        });
        let request_hash =
            RequestHash::from_request("freeze_proxy_authorization", lot_id, &request)?;
        let result_payload = serde_json::to_string(&json!({
            "authorization_id": authorization_id,
            "source_command_id": source_command_id,
            "status": "frozen",
        }))?;
        sqlx::query(
            "INSERT INTO auction_commands
                (command_id, command_kind, aggregate_id, request_hash, result_code,
                 result_payload, created_at)
             VALUES (?, 'freeze_proxy_authorization', ?, ?, 'accepted', ?, ?)",
        )
        .bind(uuid_blob(child_command_id))
        .bind(uuid_blob(lot_id))
        .bind(request_hash.as_bytes().to_vec())
        .bind(result_payload)
        .bind(&now)
        .execute(&mut *connection)
        .await
        .map_err(map_sqlite_write_error)?;

        let updated = sqlx::query(
            "UPDATE bid_proxy_authorizations
             SET status = 'frozen', terminal_reason = 'eligibility_lost',
                 state_version = ?, updated_at = ?
             WHERE authorization_id = ? AND status = ? AND state_version = ?",
        )
        .bind(next_version)
        .bind(&now)
        .bind(uuid_blob(authorization_id))
        .bind(&status)
        .bind(state_version)
        .execute(&mut *connection)
        .await
        .map_err(map_sqlite_error)?;
        if updated.rows_affected() != 1 {
            return Err(admin_error(AdminUserError::StoredResultCorrupt));
        }

        sqlx::query(
            "INSERT INTO bid_proxy_authorization_transitions
                (authorization_id, transition_number, transition_kind, origin_command_id,
                 old_maximum_amount, new_maximum_amount, old_mode, new_mode, old_status,
                 new_status, reason, resulting_state_version, created_at)
             VALUES (?, ?, 'frozen', ?, ?, ?, ?, ?, ?,
                     'frozen', 'eligibility_lost', ?, ?)",
        )
        .bind(uuid_blob(authorization_id))
        .bind(next_version)
        .bind(uuid_blob(child_command_id))
        .bind(maximum_amount)
        .bind(maximum_amount)
        .bind(&mode)
        .bind(&mode)
        .bind(&status)
        .bind(next_version)
        .bind(&now)
        .execute(&mut *connection)
        .await
        .map_err(map_sqlite_error)?;

        sqlx::query(
            "UPDATE lots
             SET slow_authorization_id = NULL, slow_due_at = NULL,
                 slow_generation = slow_generation + 1, updated_at = ?
             WHERE id = ? AND slow_authorization_id = ?",
        )
        .bind(&now)
        .bind(uuid_blob(lot_id))
        .bind(uuid_blob(authorization_id))
        .execute(&mut *connection)
        .await
        .map_err(map_sqlite_error)?;
    }

    sqlx::query(
        "UPDATE public_sessions SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL",
    )
    .bind(&now)
    .bind(user_id_blob)
    .execute(&mut *connection)
    .await
    .map_err(map_sqlite_error)?;
    Ok(())
}
