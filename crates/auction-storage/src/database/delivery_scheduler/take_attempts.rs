//! Take 发送前冻结原账号；旧未知动作不能用当前账号的同 MID 观察恢复。

use auction_application::{DeliveryTakeExpectation, DeliveryWorkClaim};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

/// 冻结账号只服务于已解码 Take 的同世界、同 MID；资产仍由完整 claim payload 拥有。
pub(super) fn matches_claim(
    claim: &DeliveryWorkClaim,
    expectation: &DeliveryTakeExpectation,
) -> bool {
    expectation.validate()
        && claim.payload.get("operation").and_then(Value::as_str) == Some("take")
        && claim.payload.get("world").and_then(Value::as_str) == Some(expectation.world_id.as_str())
        && claim.payload.get("external_mail_id").and_then(Value::as_str)
            == Some(expectation.external_mail_id.as_str())
}

/// 与 Sending 和审计一并追加；任何失败均由调用方回滚，不改写旧尝试。
pub(super) async fn freeze(
    tx: &mut Transaction<'_, Postgres>,
    claim: &DeliveryWorkClaim,
    started_at: DateTime<Utc>,
    expectation: &DeliveryTakeExpectation,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO public.delivery_take_attempts
         (delivery_id,fencing_version,command_id,request_digest,world_id,sending_at,expectation)
         VALUES ($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(claim.delivery_id)
    .bind(claim.fencing_version)
    .bind(claim.command_id)
    .bind(claim.request_hash.as_bytes().as_slice())
    .bind(&expectation.world_id)
    .bind(started_at)
    .bind(sqlx::types::Json(expectation))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// 先锁当前 delivery；过期隔离可推进 CAS fence，但原 Taking 身份沿发送签名解析。
/// 缺尝试或签名不符均返回 None，不能回填当前账号或退回无绑定旧路径。
pub(in crate::database) async fn load_current(
    tx: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
    current_fencing_version: i64,
) -> Result<Option<(i64, DeliveryTakeExpectation)>, sqlx::Error> {
    sqlx::query_as::<_, (i64, sqlx::types::Json<DeliveryTakeExpectation>)>(
        "SELECT attempt.taking_fencing_version,attempt.expectation
         FROM public.deliveries delivery
         JOIN LATERAL public.delivery_current_take_attempt(delivery.id) attempt ON true
         WHERE delivery.id=$1 AND delivery.fencing_version=$2",
    )
    .bind(delivery_id)
    .bind(current_fencing_version)
    .fetch_optional(&mut **tx)
    .await
    .map(|attempt| attempt.map(|(fencing_version, expectation)| (fencing_version, expectation.0)))
}
