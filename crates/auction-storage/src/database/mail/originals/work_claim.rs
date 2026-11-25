//! 寄售处置工作的领取写入：期 4 处置与退件 Take 两条领取共用同一判据与同一写法。
//!
//! 两条领取各自只负责“选哪一行”（互不重叠）；选中并以 `FOR UPDATE OF d SKIP LOCKED` 锁住投递后，
//! 推进 fence 与租约、绑定当前世界就绪会话登记 attempt 的写法只在这里实现一次。
//!
//! 保护不变量 4、5：领取只写持久租约与 attempt，不起写、不外写；POST 许可只由起写首次提交铸造。
//! 世界未就绪时失败关闭，整笔回滚，不推进任何持久状态。

use auction_application::consignment::{
    ConsignmentError, DepositBusinessRejection, DispositionAction,
};
use auction_application::mail::DispositionWorkClaim;
use auction_time::Timestamp;
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::disposition::manifest_from_payload;
use super::identity::parse_world;
use super::{invariant, invariant_decoding};
use crate::database::consignment::commands::errors::WriteError;

/// 可领取判据：处置投递已备妥且没有有效租约（或租约已过期），持久退避时刻已到。
///
/// 只用于以 `d` 为投递别名、并在外层物化了 `sql_clock` 的领取语句。
macro_rules! disposition_claimable {
    () => {
        "d.origin_kind = 'consignment_disposition'
           AND (
               (d.status = 'prepared'
                AND (d.lease_expires_at IS NULL
                     OR d.lease_expires_at <= (SELECT at FROM sql_clock)))
               OR (d.status = 'claimed'
                   AND d.lease_expires_at <= (SELECT at FROM sql_clock))
           )
           AND COALESCE(
                 CASE WHEN pg_input_is_valid(d.payload->>'retry_at', 'timestamptz')
                      THEN (d.payload->>'retry_at')::timestamptz END,
                 '-infinity'::timestamptz) <= (SELECT at FROM sql_clock)"
    };
}
pub(crate) use disposition_claimable;

/// 领取语句选出的列：两条领取必须原样选出这些列名，[`claim_selected_in_tx`] 按名读取。
///
/// `$1` 是租约时长（微秒，见 [`claim_lease_micros`]）。
macro_rules! disposition_claim_columns {
    () => {
        "w.id, w.decision_id, w.delivery_id, w.action, w.manifest_digest, d.payload,
         d.fencing_version, d.attempt_count,
         (SELECT at FROM sql_clock) AS claimed_at,
         (SELECT at FROM sql_clock) + ($1 * interval '1 microsecond') AS lease_until"
    };
}
pub(crate) use disposition_claim_columns;

/// 校验领取入参并换算租约时长（微秒）；时长非正或 Worker 名为空都是调用方错误。
pub(crate) fn claim_lease_micros(
    worker_id: &str,
    now: Timestamp,
    lease_until: Timestamp,
) -> Result<i64, WriteError> {
    if worker_id.is_empty() {
        return Err(invariant("consignment_work_claim_input"));
    }
    lease_until
        .signed_duration_since(now)
        .num_microseconds()
        .filter(|value| *value > 0)
        .ok_or_else(|| invariant("consignment_work_claim_input"))
}

/// 在调用方事务内领取已选中并锁住的一行工作：推进 fence、租约与尝试次数，按当前世界就绪会话
/// 登记 attempt。世界未就绪时返回 `MailUnavailable`，调用方丢弃事务即整笔回滚。
pub(crate) async fn claim_selected_in_tx(
    connection: &mut PgConnection,
    row: &PgRow,
) -> Result<DispositionWorkClaim, WriteError> {
    let work_id: Uuid = row.try_get("id")?;
    let decision_id: Uuid = row.try_get("decision_id")?;
    let delivery_id: Uuid = row.try_get("delivery_id")?;
    let action = match row.try_get::<String, _>("action")?.as_str() {
        "take" => DispositionAction::Take,
        "return" => DispositionAction::Return,
        _ => return Err(invariant("consignment_work_action")),
    };
    let manifest_digest: &[u8] = row.try_get("manifest_digest")?;
    let manifest_digest = <[u8; 32]>::try_from(manifest_digest)
        .map_err(invariant_decoding("consignment_work_manifest_digest"))?;
    let payload: serde_json::Value = row.try_get("payload")?;
    let manifest = manifest_from_payload(&payload, manifest_digest)?;
    let claim_token = Uuid::new_v4();
    let fencing: i64 = row.try_get("fencing_version")?;
    let attempt_count: i32 = row.try_get("attempt_count")?;
    let claimed_at: Timestamp = row.try_get("claimed_at")?;
    let lease_until: Timestamp = row.try_get("lease_until")?;
    let next_fence =
        fencing.checked_add(1).ok_or_else(|| invariant("consignment_work_fence_overflow"))?;
    let next_attempt = attempt_count
        .checked_add(1)
        .ok_or_else(|| invariant("consignment_work_attempt_overflow"))?;
    sqlx::query(
        "UPDATE public.deliveries
         SET status = 'claimed', lease_token = $2, lease_expires_at = $3,
             fencing_version = $4, attempt_count = $5, claimed_at = $6, updated_at = $6,
             payload = payload - 'retry_at'
         WHERE id = $1
           AND (status = 'prepared'
                OR (status = 'claimed' AND lease_expires_at <= $6))",
    )
    .bind(delivery_id)
    .bind(claim_token)
    .bind(lease_until)
    .bind(next_fence)
    .bind(next_attempt)
    .bind(claimed_at)
    .execute(&mut *connection)
    .await?;
    let world = parse_world(
        &sqlx::query_scalar::<_, String>(
            "SELECT world_id FROM public.mail_disposition_work WHERE id = $1",
        )
        .bind(work_id)
        .fetch_one(&mut *connection)
        .await?,
    )?;
    let session = sqlx::query(
        "SELECT r.session_id, s.credential_id, s.credential_version, s.runtime_generation
         FROM public.consignment_mail_readiness AS r
         JOIN public.mail_evidence_sessions AS s ON s.id = r.session_id
         WHERE r.world_id = $1 AND r.valid_until > $2",
    )
    .bind(auction_domain::consignment::settlement_world_code(world))
    .bind(claimed_at)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(session) = session else {
        return Err(WriteError::Application(ConsignmentError::Deposit(
            DepositBusinessRejection::MailUnavailable,
        )));
    };
    let attempt_id = Uuid::new_v4();
    let session_id: Uuid = session.try_get("session_id")?;
    sqlx::query(
        "INSERT INTO public.mail_disposition_attempts
            (id, work_id, delivery_id, fence, credential_id, credential_version,
             runtime_generation, session_id, expected_digest, prepared_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(attempt_id)
    .bind(work_id)
    .bind(delivery_id)
    .bind(next_fence)
    .bind(session.try_get::<Uuid, _>("credential_id")?)
    .bind(session.try_get::<i64, _>("credential_version")?)
    .bind(session.try_get::<i64, _>("runtime_generation")?)
    .bind(session_id)
    .bind(manifest_digest.as_slice())
    .bind(claimed_at)
    .execute(&mut *connection)
    .await?;
    Ok(DispositionWorkClaim {
        work_id,
        final_decision_id: decision_id,
        delivery_id,
        action,
        attempt_id,
        fence: u64::try_from(next_fence).map_err(invariant_decoding("consignment_work_fence"))?,
        claim_token,
        attempt_count: u32::try_from(next_attempt)
            .map_err(invariant_decoding("consignment_work_attempt"))?,
        lease_until,
        manifest,
    })
}
