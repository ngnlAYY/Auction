//! W4.6 玩家支持只读汇总的 PostgreSQL 读取。
//!
//! 所有分区共享同一只读快照，各自 `LIMIT 上限 + 1` 以发现截断；不写任何事实、命令或审计。
//! 参考编号只按规范 UUID 解析，且不把投递 payload、正文或凭据带出存储层。

use auction_application::delivery::{
    DeliveryAdminError, MAX_PLAYER_SUPPORT_ROWS, PlayerSupportBid, PlayerSupportBundle,
    PlayerSupportDeal, PlayerSupportNotification, PlayerSupportObligation, PlayerSupportReference,
    PlayerSupportRequest, PlayerSupportSelector, PlayerSupportUser,
};
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::map_storage_error;
use super::mapping::snapshot;
use crate::database::TrackedPool;

/// 只读汇总；未知用户或参考编号返回空分区而不是 404，便于 Admin 直接展示“无记录”。
pub(crate) async fn player_support(
    pool: &TrackedPool<Postgres>,
    request: &PlayerSupportRequest,
) -> Result<PlayerSupportBundle, DeliveryAdminError> {
    let mut transaction = pool.begin_read_snapshot().await.map_err(map_storage_error)?;
    let references = match &request.selector {
        PlayerSupportSelector::User(_) => Vec::new(),
        PlayerSupportSelector::Reference(reference) => {
            references(&mut transaction, *reference).await?
        }
    };
    let user_id = match &request.selector {
        PlayerSupportSelector::User(value) => user_id_for_selector(&mut transaction, value).await?,
        PlayerSupportSelector::Reference(reference) => {
            user_id_for_reference(&mut transaction, *reference).await?
        }
    };
    let Some(user_id) = user_id else {
        transaction.commit().await.map_err(map_storage_error)?;
        return Ok(PlayerSupportBundle { references, ..PlayerSupportBundle::default() });
    };
    let user = user_row(&mut transaction, user_id).await?;
    let game_player_id = user.as_ref().and_then(|user| user.game_player_id.clone());

    let mut truncated = false;
    let bids = bids(&mut transaction, user_id, &mut truncated).await?;
    let deals = deals(&mut transaction, user_id, &mut truncated).await?;
    let obligations = obligations(&mut transaction, user_id, &mut truncated).await?;
    let mut deliveries =
        deliveries(&mut transaction, user_id, game_player_id.as_deref(), &mut truncated).await?;
    let mut notifications = notifications(&mut transaction, user_id, &mut truncated).await?;
    // 参考编号必须能一次定位到它指向的记录，即使该记录尚未与用户分区关联。
    if let PlayerSupportSelector::Reference(reference) = &request.selector {
        attach_reference(&mut transaction, *reference, &mut deliveries, &mut notifications).await?;
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(PlayerSupportBundle {
        user,
        bids,
        deals,
        obligations,
        deliveries,
        notifications,
        references,
        truncated,
    })
}

async fn user_id_for_selector(
    transaction: &mut crate::database::TrackedTransaction<'_, Postgres>,
    value: &str,
) -> Result<Option<Uuid>, DeliveryAdminError> {
    if let Ok(id) = Uuid::parse_str(value) {
        let found: Option<Uuid> = sqlx::query_scalar("SELECT id FROM public.users WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(map_storage_error)?;
        return Ok(found);
    }
    sqlx::query_scalar("SELECT id FROM public.users WHERE lower(username) = lower($1)")
        .bind(value)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_storage_error)
}

/// 从参考编号推导关联用户；多条候选时按固定顺序取第一条，避免结果随计划变化。
async fn user_id_for_reference(
    transaction: &mut crate::database::TrackedTransaction<'_, Postgres>,
    reference: Uuid,
) -> Result<Option<Uuid>, DeliveryAdminError> {
    sqlx::query_scalar(
        r#"WITH candidates AS (
               SELECT obligation.recipient_user_id AS user_id
               FROM public.settlement_obligations AS obligation
               WHERE obligation.id = $1 AND obligation.recipient_user_id IS NOT NULL
               UNION ALL
               SELECT notification.user_id
               FROM public.lot_watch_notifications AS notification
               WHERE notification.id = $1
               UNION ALL
               SELECT pending.user_id
               FROM public.lot_watch_pending_notifications AS pending
               WHERE pending.id = $1
               UNION ALL
               SELECT obligation.recipient_user_id
               FROM public.settlement_obligation_fulfillments AS fulfillment
               JOIN public.settlement_obligations AS obligation
                 ON obligation.id = fulfillment.obligation_id
               WHERE fulfillment.delivery_id = $1
                 AND obligation.recipient_user_id IS NOT NULL
               UNION ALL
               SELECT notification.user_id
               FROM public.lot_watch_notifications AS notification
               WHERE notification.delivery_id = $1
               UNION ALL
               SELECT user_account.id
               FROM public.deliveries AS delivery
               JOIN public.users AS user_account
                 ON lower(user_account.game_player_id) = lower(delivery.recipient)
               WHERE delivery.id = $1
               UNION ALL
               SELECT lot.leader_user_id
               FROM public.lots AS lot
               WHERE lot.id = $1
           )
           SELECT user_id FROM candidates WHERE user_id IS NOT NULL LIMIT 1"#,
    )
    .bind(reference)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_storage_error)
}

async fn references(
    transaction: &mut crate::database::TrackedTransaction<'_, Postgres>,
    reference: Uuid,
) -> Result<Vec<PlayerSupportReference>, DeliveryAdminError> {
    let rows = sqlx::query(
        r#"SELECT 'delivery'::text AS kind FROM public.deliveries WHERE id = $1
           UNION ALL
           SELECT 'obligation'::text FROM public.settlement_obligations WHERE id = $1
           UNION ALL
           SELECT 'notification'::text FROM public.lot_watch_notifications WHERE id = $1
           UNION ALL
           SELECT 'notification'::text FROM public.lot_watch_pending_notifications WHERE id = $1
           UNION ALL
           SELECT 'lot'::text FROM public.lots WHERE id = $1
           UNION ALL
           SELECT 'auction'::text FROM public.auctions WHERE id = $1"#,
    )
    .bind(reference)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    Ok(rows
        .into_iter()
        .map(|row| PlayerSupportReference { kind: row.get::<String, _>("kind"), id: reference })
        .collect())
}

async fn user_row(
    transaction: &mut crate::database::TrackedTransaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<Option<PlayerSupportUser>, DeliveryAdminError> {
    let row =
        sqlx::query("SELECT id, username, status, game_player_id FROM public.users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(map_storage_error)?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(PlayerSupportUser {
        id: row.get("id"),
        username: row.get("username"),
        status: row.get("status"),
        game_player_id: row.get("game_player_id"),
    }))
}

async fn bids(
    transaction: &mut crate::database::TrackedTransaction<'_, Postgres>,
    user_id: Uuid,
    truncated: &mut bool,
) -> Result<Vec<PlayerSupportBid>, DeliveryAdminError> {
    let rows = sqlx::query(
        r#"SELECT bid.id, bid.lot_id, lot.auction_id, lot.lot_number::bigint AS lot_number,
                  auction.world_id, bid.amount, bid.mechanism, lot.status AS lot_status,
                  bid.created_at
           FROM public.bids AS bid
           JOIN public.lots AS lot ON lot.id = bid.lot_id
           JOIN public.auctions AS auction ON auction.id = lot.auction_id
           WHERE bid.bidder_user_id = $1
           ORDER BY bid.created_at DESC, bid.id DESC
           LIMIT $2"#,
    )
    .bind(user_id)
    .bind(MAX_PLAYER_SUPPORT_ROWS + 1)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    let mut entries = Vec::new();
    for row in rows {
        if entries.len() as i64 == MAX_PLAYER_SUPPORT_ROWS {
            *truncated = true;
            break;
        }
        entries.push(PlayerSupportBid {
            bid_id: row.get("id"),
            lot_id: row.get("lot_id"),
            auction_id: row.get("auction_id"),
            lot_number: row.get("lot_number"),
            world_id: row.get("world_id"),
            amount: row.get("amount"),
            mechanism: row.get("mechanism"),
            lot_status: row.get("lot_status"),
            created_at: row.get("created_at"),
        });
    }
    Ok(entries)
}

async fn deals(
    transaction: &mut crate::database::TrackedTransaction<'_, Postgres>,
    user_id: Uuid,
    truncated: &mut bool,
) -> Result<Vec<PlayerSupportDeal>, DeliveryAdminError> {
    let rows = sqlx::query(
        r#"SELECT lot.id AS lot_id, lot.auction_id, lot.lot_number::bigint AS lot_number,
                  auction.world_id,
                  equipment.name AS equipment_name, lot.current_price, lot.effective_end_at
           FROM public.lots AS lot
           JOIN public.auctions AS auction ON auction.id = lot.auction_id
           JOIN public.equipment AS equipment ON equipment.id = lot.equipment_id
           WHERE lot.leader_user_id = $1
             AND lot.status = 'sold'
             AND lot.current_price IS NOT NULL
           ORDER BY lot.effective_end_at DESC NULLS LAST, lot.id DESC
           LIMIT $2"#,
    )
    .bind(user_id)
    .bind(MAX_PLAYER_SUPPORT_ROWS + 1)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    let mut entries = Vec::new();
    for row in rows {
        if entries.len() as i64 == MAX_PLAYER_SUPPORT_ROWS {
            *truncated = true;
            break;
        }
        entries.push(PlayerSupportDeal {
            lot_id: row.get("lot_id"),
            auction_id: row.get("auction_id"),
            lot_number: row.get("lot_number"),
            world_id: row.get("world_id"),
            equipment_name: row.get("equipment_name"),
            final_price: row.get("current_price"),
            effective_end_at: row.get("effective_end_at"),
        });
    }
    Ok(entries)
}

async fn obligations(
    transaction: &mut crate::database::TrackedTransaction<'_, Postgres>,
    user_id: Uuid,
    truncated: &mut bool,
) -> Result<Vec<PlayerSupportObligation>, DeliveryAdminError> {
    let rows = sqlx::query(
        r#"SELECT obligation.id, obligation.auction_id, obligation.lot_id, obligation.world_id,
                  obligation.obligation_kind, obligation.business_state, obligation.amount,
                  fulfillment.execution_state, fulfillment.last_error_code,
                  fulfillment.delivery_id
           FROM public.settlement_obligations AS obligation
           LEFT JOIN public.settlement_obligation_fulfillments AS fulfillment
             ON fulfillment.obligation_id = obligation.id
           WHERE obligation.recipient_user_id = $1
           ORDER BY obligation.created_at DESC, obligation.id DESC
           LIMIT $2"#,
    )
    .bind(user_id)
    .bind(MAX_PLAYER_SUPPORT_ROWS + 1)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    let mut entries = Vec::new();
    for row in rows {
        if entries.len() as i64 == MAX_PLAYER_SUPPORT_ROWS {
            *truncated = true;
            break;
        }
        entries.push(PlayerSupportObligation {
            obligation_id: row.get("id"),
            auction_id: row.get("auction_id"),
            lot_id: row.get("lot_id"),
            world_id: row.get("world_id"),
            obligation_kind: row.get("obligation_kind"),
            business_state: row.get("business_state"),
            amount: row.get("amount"),
            fulfillment_state: row.get("execution_state"),
            fulfillment_error_code: row.get("last_error_code"),
            delivery_id: row.get("delivery_id"),
        });
    }
    Ok(entries)
}

async fn deliveries(
    transaction: &mut crate::database::TrackedTransaction<'_, Postgres>,
    user_id: Uuid,
    game_player_id: Option<&str>,
    truncated: &mut bool,
) -> Result<Vec<auction_application::delivery::DeliveryAdminSnapshot>, DeliveryAdminError> {
    let rows = sqlx::query(
        r#"SELECT id, origin_kind, origin_id, recipient, payload, result_metadata, status,
                  lease_token, lease_expires_at, fencing_version, attempt_count, claimed_at,
                  sending_at, completed_at, external_reference, failure_code,
                  created_at, updated_at
           FROM public.deliveries
           WHERE id IN (
               SELECT fulfillment.delivery_id
               FROM public.settlement_obligation_fulfillments AS fulfillment
               JOIN public.settlement_obligations AS obligation
                 ON obligation.id = fulfillment.obligation_id
               WHERE obligation.recipient_user_id = $1
                 AND fulfillment.delivery_id IS NOT NULL
               UNION
               SELECT notification.delivery_id
               FROM public.lot_watch_notifications AS notification
               WHERE notification.user_id = $1
                 AND notification.delivery_id IS NOT NULL
               UNION
               SELECT delivery.id
               FROM public.deliveries AS delivery
               WHERE $2::text IS NOT NULL
                 AND lower(delivery.recipient) = lower($2::text)
           )
           ORDER BY created_at DESC, id DESC
           LIMIT $3"#,
    )
    .bind(user_id)
    .bind(game_player_id)
    .bind(MAX_PLAYER_SUPPORT_ROWS + 1)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    let mut entries = Vec::new();
    for row in rows {
        if entries.len() as i64 == MAX_PLAYER_SUPPORT_ROWS {
            *truncated = true;
            break;
        }
        entries.push(snapshot(&row)?);
    }
    Ok(entries)
}

async fn notifications(
    transaction: &mut crate::database::TrackedTransaction<'_, Postgres>,
    user_id: Uuid,
    truncated: &mut bool,
) -> Result<Vec<PlayerSupportNotification>, DeliveryAdminError> {
    let rows = sqlx::query(
        r#"SELECT notification.id AS notification_id, 'mm'::text AS channel,
                  CASE WHEN notification.channel = 'mm' THEN 'lot_watch'
                       ELSE 'lot_watch_pm' END AS kind,
                  notification.lot_id, lot.auction_id,
                  COALESCE(delivery.status, 'pending') AS state,
                  COALESCE(delivery.result_metadata ->> 'reason_code', delivery.failure_code)
                      AS state_reason,
                  notification.delivery_id,
                  notification.created_at,
                  COALESCE(delivery.updated_at, notification.created_at) AS updated_at
           FROM public.lot_watch_notifications AS notification
           JOIN public.lots AS lot ON lot.id = notification.lot_id
           LEFT JOIN public.deliveries AS delivery ON delivery.id = notification.delivery_id
           WHERE notification.user_id = $1 AND notification.channel = 'mm'
           UNION ALL
           SELECT notification.id, 'pm'::text, 'lot_watch_pm'::text,
                  notification.lot_id, lot.auction_id,
                  pm.status, pm.failure_code, NULL::uuid,
                  notification.created_at, pm.updated_at
           FROM public.lot_watch_notifications AS notification
           JOIN public.lots AS lot ON lot.id = notification.lot_id
           JOIN public.lot_watch_pm_deliveries AS pm
             ON pm.notification_id = notification.id
           WHERE notification.user_id = $1 AND notification.channel = 'pm'
           UNION ALL
           SELECT pending.id, 'pending'::text,
                  CASE WHEN pending.channel = 'mm' THEN 'lot_watch'
                       ELSE 'lot_watch_pm' END,
                  pending.lot_id, lot.auction_id,
                  pending.status, pending.resolution_reason, NULL::uuid,
                  pending.created_at, COALESCE(pending.resolved_at, pending.created_at)
           FROM public.lot_watch_pending_notifications AS pending
           JOIN public.lots AS lot ON lot.id = pending.lot_id
           WHERE pending.user_id = $1
           ORDER BY created_at DESC, notification_id DESC
           LIMIT $2"#,
    )
    .bind(user_id)
    .bind(MAX_PLAYER_SUPPORT_ROWS + 1)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    let mut entries = Vec::new();
    for row in rows {
        if entries.len() as i64 == MAX_PLAYER_SUPPORT_ROWS {
            *truncated = true;
            break;
        }
        entries.push(PlayerSupportNotification {
            notification_id: row.get("notification_id"),
            channel: row.get("channel"),
            kind: row.get("kind"),
            lot_id: row.get("lot_id"),
            auction_id: row.get("auction_id"),
            state: row.get("state"),
            state_reason: row.get("state_reason"),
            delivery_id: row.get("delivery_id"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        });
    }
    Ok(entries)
}

/// 把参考编号自身命中的投递或通知补进分区，已存在时不重复。
async fn attach_reference(
    transaction: &mut crate::database::TrackedTransaction<'_, Postgres>,
    reference: Uuid,
    deliveries: &mut Vec<auction_application::delivery::DeliveryAdminSnapshot>,
    notifications: &mut Vec<PlayerSupportNotification>,
) -> Result<(), DeliveryAdminError> {
    if !deliveries.iter().any(|delivery| delivery.id == reference) {
        let row = sqlx::query(
            r#"SELECT id, origin_kind, origin_id, recipient, payload, result_metadata, status,
                      lease_token, lease_expires_at, fencing_version, attempt_count, claimed_at,
                      sending_at, completed_at, external_reference, failure_code,
                      created_at, updated_at
               FROM public.deliveries WHERE id = $1"#,
        )
        .bind(reference)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_storage_error)?;
        if let Some(row) = row {
            deliveries.insert(0, snapshot(&row)?);
        }
    }
    if !notifications.iter().any(|item| item.notification_id == reference) {
        let row = sqlx::query(
            r#"SELECT notification.id AS notification_id,
                      CASE WHEN notification.channel = 'mm' THEN 'mm' ELSE 'pm' END AS channel,
                      CASE WHEN notification.channel = 'mm' THEN 'lot_watch'
                           ELSE 'lot_watch_pm' END AS kind,
                      notification.lot_id, lot.auction_id,
                      COALESCE(delivery.status, pm.status) AS state,
                      COALESCE(pm.failure_code, delivery.result_metadata ->> 'reason_code',
                               delivery.failure_code) AS state_reason,
                      notification.delivery_id,
                      notification.created_at,
                      GREATEST(notification.created_at, COALESCE(delivery.updated_at, pm.updated_at,
                                                               notification.created_at))
                          AS updated_at
               FROM public.lot_watch_notifications AS notification
               JOIN public.lots AS lot ON lot.id = notification.lot_id
               LEFT JOIN public.deliveries AS delivery ON delivery.id = notification.delivery_id
               LEFT JOIN public.lot_watch_pm_deliveries AS pm
                 ON pm.notification_id = notification.id
               WHERE notification.id = $1"#,
        )
        .bind(reference)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_storage_error)?;
        if let Some(row) = row {
            notifications.insert(
                0,
                PlayerSupportNotification {
                    notification_id: row.get("notification_id"),
                    channel: row.get("channel"),
                    kind: row.get("kind"),
                    lot_id: row.get("lot_id"),
                    auction_id: row.get("auction_id"),
                    state: row.get("state"),
                    state_reason: row.get("state_reason"),
                    delivery_id: row.get("delivery_id"),
                    created_at: row.get("created_at"),
                    updated_at: row.get("updated_at"),
                },
            );
        }
    }
    Ok(())
}
