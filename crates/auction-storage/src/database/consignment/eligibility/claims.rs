//! 寄售实物强占用的无空档持有、释放与转 position。
//!
//! 所有变化都在调用方已持有 `(world, canonical_eid)` session 锁与当前事务内完成；
//! 投影触发器会再核对预约/position/事件是否来自同一顶层事务。

use auction_domain::consignment::ListingEquipmentIdentity;
use auction_time::Timestamp;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::database::consignment::commands::errors::WriteError;

/// 把空闲或被同一 listing 旧预约持有的实物转为新预约持有。
pub(crate) async fn hold_reservation(
    connection: &mut PgConnection,
    identity: &ListingEquipmentIdentity,
    reservation_id: Uuid,
    now: Timestamp,
) -> Result<(), WriteError> {
    let result = sqlx::query(
        "INSERT INTO public.consignment_equipment_claims
            (world_id, canonical_eid, revision, reservation_id, position_id, changed_at,
             last_fact_id)
         VALUES ($1, $2, 1, $3, NULL, $4, $3)
         ON CONFLICT (world_id, canonical_eid) DO UPDATE
         SET reservation_id = excluded.reservation_id,
             position_id = NULL,
             revision = public.consignment_equipment_claims.revision + 1,
             changed_at = excluded.changed_at,
             last_fact_id = excluded.reservation_id
         WHERE public.consignment_equipment_claims.position_id IS NULL
           AND (
               public.consignment_equipment_claims.reservation_id IS NULL
               OR public.consignment_equipment_claims.reservation_id = excluded.reservation_id
               OR EXISTS (
                   SELECT 1 FROM public.consignment_deposit_reservations AS previous
                   WHERE previous.id = public.consignment_equipment_claims.reservation_id
                     AND previous.state <> 'active'
               )
           )",
    )
    .bind(identity.world_id())
    .bind(identity.external_equipment_id())
    .bind(reservation_id)
    .bind(now)
    .execute(connection)
    .await?;
    if result.rows_affected() != 1 {
        return Err(auction_application::consignment::ConsignmentError::Deposit(
            auction_application::consignment::DepositBusinessRejection::EquipmentBusy,
        )
        .into());
    }
    Ok(())
}

/// 预约退休后释放强占用。
pub(crate) async fn release_reservation(
    connection: &mut PgConnection,
    identity: &ListingEquipmentIdentity,
    reservation_id: Uuid,
    now: Timestamp,
) -> Result<(), WriteError> {
    sqlx::query(
        "UPDATE public.consignment_equipment_claims
         SET reservation_id = NULL, revision = revision + 1, changed_at = $4,
             last_fact_id = $3
         WHERE world_id = $1 AND canonical_eid = $2 AND reservation_id = $3",
    )
    .bind(identity.world_id())
    .bind(identity.external_equipment_id())
    .bind(reservation_id)
    .bind(now)
    .execute(connection)
    .await?;
    Ok(())
}

/// 整封接受时把预约持有无空档转为 position 持有。
pub(crate) async fn transfer_to_position(
    connection: &mut PgConnection,
    identity: &ListingEquipmentIdentity,
    reservation_id: Uuid,
    position_id: Uuid,
    now: Timestamp,
) -> Result<(), WriteError> {
    let result = sqlx::query(
        "UPDATE public.consignment_equipment_claims
         SET reservation_id = NULL, position_id = $4, revision = revision + 1,
             changed_at = $5, last_fact_id = $4
         WHERE world_id = $1 AND canonical_eid = $2 AND reservation_id = $3
           AND position_id IS NULL",
    )
    .bind(identity.world_id())
    .bind(identity.external_equipment_id())
    .bind(reservation_id)
    .bind(position_id)
    .bind(now)
    .execute(connection)
    .await?;
    if result.rows_affected() != 1 {
        return Err(auction_application::consignment::ConsignmentError::Deposit(
            auction_application::consignment::DepositBusinessRejection::EquipmentBusy,
        )
        .into());
    }
    Ok(())
}

/// 挂单命令取消仍在等待寄件的预约。
///
/// 不变量 2、6、7：取消事实只能绑定本事务的 `withdraw` / `admin_delist` 关闭命令；
/// 预约、令牌密文与实物强占用同事务释放，任何一步失败都由外层回滚。
pub(crate) async fn cancel_active_reservations(
    conn: &mut PgConnection,
    listing_id: Uuid,
    command_id: Uuid,
    now: Timestamp,
) -> Result<(), WriteError> {
    // 只取实物身份；卖家列自 000138 起可空（匿名非寄售挂单没有账户），此处从未使用它。
    let identity: (String, String) = sqlx::query_as(
        "SELECT world_id, external_equipment_id
         FROM public.consignment_listings WHERE id = $1",
    )
    .bind(listing_id)
    .fetch_one(&mut *conn)
    .await?;
    let reservation_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM public.consignment_deposit_reservations
         WHERE listing_id = $1 AND state = 'active' ORDER BY id FOR UPDATE",
    )
    .bind(listing_id)
    .fetch_all(&mut *conn)
    .await?;
    if reservation_ids.is_empty() {
        return Ok(());
    }
    let identity = ListingEquipmentIdentity::new(&identity.0, &identity.1)?;
    for reservation_id in reservation_ids {
        sqlx::query(
            "UPDATE public.consignment_deposit_reservations
             SET state = 'retired', retired_at = $2, reason = 'cancelled',
                 last_command_id = $3
             WHERE id = $1 AND state = 'active'",
        )
        .bind(reservation_id)
        .bind(now)
        .bind(command_id)
        .execute(&mut *conn)
        .await?;
        // 不变量 7：取消后的预约码不能继续被读取或重放为寄件指引。
        sqlx::query(
            "UPDATE public.consignment_reservation_secrets
             SET ciphertext = NULL, nonce = NULL, encryption_key_id = NULL,
                 fingerprint = NULL, retired_at = $2, updated_at = $2
             WHERE reservation_id = $1 AND retired_at IS NULL",
        )
        .bind(reservation_id)
        .bind(now)
        .execute(&mut *conn)
        .await?;
        release_reservation(&mut *conn, &identity, reservation_id, now).await?;
    }
    Ok(())
}
