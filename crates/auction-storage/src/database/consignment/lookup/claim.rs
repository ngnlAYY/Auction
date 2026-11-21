//! 锁定全局许可后领取到期解析工作，并读取登记信封。

use auction_application::consignment::{
    ListingCredentialError, ListingLookupClaim, ListingRegistrationSecret, LookupClaimRequest,
    LookupFence,
};
use auction_domain::consignment::ListingEquipmentIdentity;
use auction_domain::{EquipmentId, EquipmentKeyEnvelope, EquipmentKeyEnvelopeError};
use auction_time::Timestamp;
use sqlx::PgConnection;
use uuid::Uuid;

use super::admission::{db_now, lock_pause, paused_at};
use super::control::lock_control;
use super::protocol::{LEASE_GRACE, MAX_LISTING_LOOKUP_ATTEMPTS};
use crate::database::consignment::commands::errors::{WriteError, invalid_projection};
use crate::database::consignment::eligibility::predicates::listing_sql;

pub(super) async fn claim(
    conn: &mut PgConnection,
    request: &LookupClaimRequest,
) -> Result<Option<ListingLookupClaim>, WriteError> {
    let control = lock_control(conn).await?;
    if !control.admitting || control.enabled_epoch.cast_unsigned() != request.service_epoch() {
        return Ok(None);
    }
    lock_pause(conn).await?;
    let now = db_now(conn).await?;
    if control.next_allowed_at > now
        || control.cooldown_until.is_some_and(|until| until > now)
        || control.permit_until.is_some_and(|until| until > now)
        || paused_at(conn, now).await?
    {
        return Ok(None);
    }
    // 不变量 2、5、6：持单例锁时只锁工作，挂单和卖家按同一语句快照读。
    let candidate: Option<(Uuid, Uuid, i64, String, String)> = sqlx::query_as(concat!(
        "SELECT w.id, w.listing_id, w.generation, l.world_id, l.external_equipment_id
         FROM public.consignment_lookup_work w JOIN public.consignment_listings l ON l.id = w.listing_id",
        listing_sql!(eligibility_joins),
        "WHERE w.state = 'queued' AND w.next_attempt_at <= $1 AND w.attempt_count < $2
         AND l.state = 'resolving' AND ", listing_sql!(effective_active, "$1"),
        " ORDER BY w.next_attempt_at, w.id LIMIT 1 FOR UPDATE OF w SKIP LOCKED",
    ))
    .bind(now)
    .bind(i16::from(MAX_LISTING_LOOKUP_ATTEMPTS))
    .fetch_optional(&mut *conn)
    .await?;
    let Some((work_id, listing_id, generation, world, eid)) = candidate else {
        return Ok(None);
    };
    let identity = ListingEquipmentIdentity::new(&world, &eid).map_err(invalid_projection)?;
    let budget = request.budget();
    // 不变量 2、5：最后一次取墙钟计算两道实际截止；工作、计次与全局许可要么同提交，
    // 要么一起回滚。资格仍只做快照读，发送前用新快照复核。
    let grant: Option<(i16, i64, Timestamp, Timestamp)> = sqlx::query_as(concat!(
        "WITH db_clock AS MATERIALIZED (SELECT clock_timestamp() AS at),
         candidate AS (
             SELECT at, LEAST(l.expires_at, at + make_interval(secs => $4)) AS deadline
             FROM public.consignment_listings l CROSS JOIN db_clock",
        listing_sql!(eligibility_joins),
        "WHERE l.id = $2 AND l.state = 'resolving' AND ", listing_sql!(effective_active, "at"),
        "), granted AS (
             UPDATE public.consignment_lookup_work w
             SET state = 'leased', attempt_count = attempt_count + 1, fence = fence + 1,
                 token = $3, lease_owner = $5, network_deadline = deadline,
                 lease_until = deadline + make_interval(secs => $6), next_attempt_at = NULL,
                 updated_at = at
             FROM candidate WHERE w.id = $1
             RETURNING w.attempt_count, w.fence, w.network_deadline, w.lease_until, at
         )
         UPDATE public.consignment_lookup_control c
         SET holder_work_id = $1, holder_token = $3, holder_fence = granted.fence,
             permit_until = granted.lease_until, next_allowed_at = at + make_interval(secs => $7)
         FROM granted WHERE c.id = 1
         RETURNING granted.attempt_count, granted.fence, granted.network_deadline, granted.lease_until",
    ))
    .bind(work_id)
    .bind(listing_id)
    .bind(request.token())
    .bind(budget.timeout().as_secs_f64())
    .bind(request.worker())
    .bind(LEASE_GRACE.as_secs_f64())
    .bind(budget.min_interval().as_secs_f64())
    .fetch_optional(&mut *conn)
    .await?;
    let Some((attempt, fence, deadline, lease_until)) = grant else {
        return Ok(None);
    };
    let fence =
        LookupFence::new(work_id, listing_id, generation.cast_unsigned(), fence, request.token())?;
    let attempt = u8::try_from(attempt).map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
    Ok(Some(ListingLookupClaim::new(
        fence,
        identity,
        request.service_epoch(),
        attempt,
        deadline,
        lease_until,
        credential(conn, listing_id).await?,
    )?))
}

#[derive(sqlx::FromRow)]
struct CredentialRow {
    aad_equipment_id: Uuid,
    ciphertext: Vec<u8>,
    nonce: Vec<u8>,
    encryption_key_id: String,
    fingerprint: String,
}

async fn credential(
    conn: &mut PgConnection,
    listing_id: Uuid,
) -> Result<Result<ListingRegistrationSecret, ListingCredentialError>, WriteError> {
    let row = sqlx::query_as::<_, CredentialRow>(
        "SELECT aad_equipment_id, ciphertext, nonce, encryption_key_id, fingerprint
         FROM public.consignment_registration_secrets WHERE listing_id = $1 AND retired_at IS NULL",
    )
    .bind(listing_id)
    .fetch_optional(conn)
    .await?;
    // 不变量 7：缺失或无效信封只返回安全错误，本端口不负责开封。
    Ok(match row {
        Some(row) => EquipmentKeyEnvelope::new(
            row.ciphertext,
            row.nonce,
            row.encryption_key_id,
            row.fingerprint,
        )
        .map_err(|EquipmentKeyEnvelopeError::InvalidShape| ListingCredentialError)
        .and_then(|envelope| {
            ListingRegistrationSecret::new(EquipmentId::from_uuid(row.aad_equipment_id), envelope)
        }),
        None => Err(ListingCredentialError),
    })
}
