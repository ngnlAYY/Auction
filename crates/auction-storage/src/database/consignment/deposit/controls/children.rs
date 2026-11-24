//! 代管控制的逐 position 子事实写入。

use uuid::Uuid;

use crate::database::consignment::commands::errors::WriteError;

/// 当前未解除的撤回意愿。
pub(super) async fn active_intent(
    connection: &mut sqlx::PgConnection,
    listing_id: Uuid,
) -> Result<Option<Uuid>, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT control.id
         FROM public.consignment_custody_controls control
         JOIN public.consignment_custody_positions position ON position.id = control.position_id
         WHERE position.listing_id = $1 AND control.kind = 'withdrawal_intent'
         ORDER BY control.version ASC LIMIT 1",
    )
    .bind(listing_id)
    .fetch_optional(connection)
    .await?)
}

/// 挂单下全部 position，按 UUID 排序。
pub(super) async fn listing_positions(
    connection: &mut sqlx::PgConnection,
    listing_id: Uuid,
) -> Result<Vec<Uuid>, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM public.consignment_custody_positions
         WHERE listing_id = $1 ORDER BY id",
    )
    .bind(listing_id)
    .fetch_all(connection)
    .await?)
}

pub(super) async fn insert_intent_controls(
    connection: &mut sqlx::PgConnection,
    listing_id: Uuid,
    primary_intent_id: Option<Uuid>,
    command_id: Uuid,
    actor_ref: &str,
    now: auction_time::Timestamp,
) -> Result<(), WriteError> {
    let positions = listing_positions(connection, listing_id).await?;
    let mut primary_available = primary_intent_id;
    for position_id in positions {
        let existing: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM public.consignment_custody_controls
             WHERE position_id = $1 AND kind = 'withdrawal_intent'",
        )
        .bind(position_id)
        .fetch_optional(&mut *connection)
        .await?;
        if existing.is_some() {
            continue;
        }
        let id = primary_available.take().unwrap_or_else(Uuid::new_v4);
        let version = next_control_version(connection, position_id).await?;
        sqlx::query(
            "INSERT INTO public.consignment_custody_controls
                (id, position_id, kind, predecessor_id, command_id, actor_ref, version,
                 occurred_at)
             VALUES ($1, $2, 'withdrawal_intent', NULL, $3, $4, $5, $6)",
        )
        .bind(id)
        .bind(position_id)
        .bind(command_id)
        .bind(actor_ref)
        .bind(version)
        .bind(now)
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

pub(super) async fn insert_hold_controls(
    connection: &mut sqlx::PgConnection,
    positions: &[Uuid],
    hold_id: Uuid,
    command_id: Uuid,
    actor_ref: &str,
    now: auction_time::Timestamp,
) -> Result<(), WriteError> {
    for (index, position_id) in positions.iter().enumerate() {
        let id = if index == 0 { hold_id } else { Uuid::new_v4() };
        let version = next_control_version(connection, *position_id).await?;
        sqlx::query(
            "INSERT INTO public.consignment_custody_controls
                (id, position_id, kind, predecessor_id, command_id, actor_ref, version,
                 occurred_at)
             VALUES ($1, $2, 'hold_set', NULL, $3, $4, $5, $6)",
        )
        .bind(id)
        .bind(position_id)
        .bind(command_id)
        .bind(actor_ref)
        .bind(version)
        .bind(now)
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

pub(super) async fn release_hold_group(
    connection: &mut sqlx::PgConnection,
    listing_id: Uuid,
    hold_command_id: Uuid,
    command_id: Uuid,
    actor_ref: &str,
    now: auction_time::Timestamp,
) -> Result<(), WriteError> {
    let holds: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT hold.id, hold.position_id
         FROM public.consignment_custody_controls hold
         JOIN public.consignment_custody_positions position ON position.id = hold.position_id
         WHERE position.listing_id = $1 AND hold.kind = 'hold_set'
           AND hold.command_id = $2
           AND NOT EXISTS (
               SELECT 1 FROM public.consignment_custody_controls release
               WHERE release.predecessor_id = hold.id)",
    )
    .bind(listing_id)
    .bind(hold_command_id)
    .fetch_all(&mut *connection)
    .await?;
    for (hold_id, position_id) in holds {
        let version = next_control_version(connection, position_id).await?;
        sqlx::query(
            "INSERT INTO public.consignment_custody_controls
                (id, position_id, kind, predecessor_id, command_id, actor_ref, version,
                 occurred_at)
             VALUES ($1, $2, 'hold_released', $3, $4, $5, $6, $7)",
        )
        .bind(Uuid::new_v4())
        .bind(position_id)
        .bind(hold_id)
        .bind(command_id)
        .bind(actor_ref)
        .bind(version)
        .bind(now)
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

async fn next_control_version(
    connection: &mut sqlx::PgConnection,
    position_id: Uuid,
) -> Result<i64, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(max(version), 0) + 1
         FROM public.consignment_custody_controls WHERE position_id = $1",
    )
    .bind(position_id)
    .fetch_one(connection)
    .await?)
}
