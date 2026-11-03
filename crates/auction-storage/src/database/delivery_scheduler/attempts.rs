//! 冻结 Send 身份及跨正常收口/维护恢复共享的唯一 Sent MID 绑定。

use crate::database::TrackedTransaction as Transaction;
use auction_application::{
    DeliverySendCredentialBinding, DeliverySendExpectation, DeliveryWorkClaim,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::Postgres;
use uuid::Uuid;

/// 非 Send 不能夹带身份；Send 必须与已锁定 claim 的世界、收件人及金额一致。
pub(super) fn matches_claim(
    claim: &DeliveryWorkClaim,
    expectation: Option<&DeliverySendExpectation>,
) -> bool {
    if claim.payload.get("operation").and_then(Value::as_str) != Some("send") {
        return expectation.is_none();
    }
    expectation.is_some_and(|expectation| {
        expectation.validate()
            && expectation.recipient == claim.recipient
            && claim.payload.get("world").and_then(Value::as_str)
                == Some(expectation.world_id.as_str())
            && claim.payload.get("credits").map_or(Some(0), Value::as_i64)
                == Some(expectation.credits)
            && claim.payload.get("cod").map_or(Some(0), Value::as_i64) == Some(expectation.cod)
    })
}

/// 按既有 funding/lot→credential→delivery 顺序锁住 active 版本；配置 Cookie 不伪造数据库身份。
pub(in crate::database) async fn lock_credential(
    tx: &mut Transaction<'_, Postgres>,
    expectation: &DeliverySendExpectation,
) -> Result<bool, sqlx::Error> {
    lock_credential_binding(tx, &expectation.world_id, expectation.credential.as_ref()).await
}

/// Send 与 Take 共用 active 凭据锁验；调用者负责冻结各自不同的请求身份。
pub(in crate::database) async fn lock_credential_binding(
    tx: &mut Transaction<'_, Postgres>,
    world_id: &str,
    binding: Option<&DeliverySendCredentialBinding>,
) -> Result<bool, sqlx::Error> {
    let Some(binding) = binding else {
        return Ok(true);
    };
    let row: Option<(String, String, String, i64)> = sqlx::query_as(
        "SELECT credential_kind,status,scope,state_version
         FROM public.credential_keys WHERE id=$1 FOR UPDATE",
    )
    .bind(binding.credential_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((kind, status, scope, version)) = row else {
        return Ok(false);
    };
    let world = match world_id {
        "main" => auction_application::AdminCredentialWorld::Main,
        "isekai" => auction_application::AdminCredentialWorld::Isekai,
        _ => return Ok(false),
    };
    let credential_kind = auction_application::AdminCredentialKind::MailCookie;
    Ok(kind == "mail_cookie"
        && status == "active"
        && version == binding.credential_version
        && (scope == world.scope(credential_kind)
            || scope == auction_application::AdminCredentialWorld::Shared.scope(credential_kind)))
}

/// 在 sending/cadence/audit 的同一事务追加；冲突不能改写旧尝试。
pub(super) async fn freeze(
    tx: &mut Transaction<'_, Postgres>,
    claim: &DeliveryWorkClaim,
    started_at: DateTime<Utc>,
    expectation: &DeliverySendExpectation,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO public.delivery_send_attempts
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

/// 先锁 delivery 再读取不可变尝试；管理员重试后不能用旧 fence 的身份关闭新尝试。
pub(in crate::database) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
    fencing_version: i64,
) -> Result<Option<DeliverySendExpectation>, sqlx::Error> {
    sqlx::query_scalar::<_, sqlx::types::Json<DeliverySendExpectation>>(
        "SELECT expectation FROM public.delivery_send_attempts
         WHERE delivery_id=$1 AND fencing_version=$2",
    )
    .bind(delivery_id)
    .bind(fencing_version)
    .fetch_optional(&mut **tx)
    .await
    .map(|expectation| expectation.map(|expectation| expectation.0))
}

/// 恢复使用当前 CAS fence，但不可变发送身份沿原 sending_at/命令/摘要解析。
/// 已有新证据却无法解析当前尝试时失败关闭，不能降级成历史无身份路径。
pub(in crate::database) async fn load_current(
    tx: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
    current_fencing_version: i64,
) -> Result<Option<(i64, Option<DeliverySendExpectation>)>, sqlx::Error> {
    sqlx::query_as::<_, (i64, Option<sqlx::types::Json<DeliverySendExpectation>>)>(
        "SELECT COALESCE(attempt.sending_fencing_version,delivery.fencing_version),attempt.expectation
         FROM public.deliveries delivery
         LEFT JOIN LATERAL public.delivery_current_send_attempt(delivery.id) attempt ON true
         WHERE delivery.id=$1 AND delivery.fencing_version=$2
           AND (attempt.sending_fencing_version IS NOT NULL OR NOT EXISTS (
               SELECT 1 FROM public.delivery_send_attempts history WHERE history.delivery_id=$1))",
    )
    .bind(delivery_id)
    .bind(current_fencing_version)
    .fetch_optional(&mut **tx)
    .await
    .map(|attempt| {
        attempt.map(|(fencing_version, expectation)| {
            (fencing_version, expectation.map(|expectation| expectation.0))
        })
    })
}

/// 即时证明和维护证明均携带实际冻结身份，不接受仅有一个布尔成功标记。
pub(in crate::database) fn matches_proof(
    metadata: &Value,
    expectation: &DeliverySendExpectation,
    fencing_version: i64,
) -> bool {
    expectation.validate()
        && metadata.get("operation").and_then(Value::as_str) == Some("send")
        && metadata.get("external_reference_proven").and_then(Value::as_bool) == Some(true)
        && metadata.get("sent_verified").and_then(Value::as_bool) == Some(true)
        && metadata.get("proof").and_then(Value::as_str) == Some("sent_exact_payload_v1")
        && metadata.get("sending_fencing_version").and_then(Value::as_i64) == Some(fencing_version)
        && serde_json::to_value(expectation)
            .ok()
            .is_some_and(|expected| metadata.get("send_expectation") == Some(&expected))
}

/// 一个 world/MID 只能证明一个 delivery/fence；同一绑定重放不产生第二条事实。
/// 调用方必须在同一 Serializable 事务内完成状态、资金和审计写入。
pub(in crate::database) async fn bind_sent(
    tx: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
    fencing_version: i64,
    world_id: &str,
    external_mail_id: &str,
    has_expectation: bool,
) -> Result<bool, sqlx::Error> {
    if !matches!(world_id, "main" | "isekai")
        || !auction_domain::valid_positive_decimal(external_mail_id, 64)
        || fencing_version <= 0
    {
        return Ok(false);
    }
    let reference = format!("mooglemail:{world_id}:send:{external_mail_id}");
    // 升级前成功行可能没有新绑定，仍须排除；SSI 保留跨事务并发插入的谓词冲突。
    let legacy_collision: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.deliveries
         WHERE external_reference=$1 AND (id<>$2 OR fencing_version<>$3))",
    )
    .bind(reference)
    .bind(delivery_id)
    .bind(fencing_version)
    .fetch_one(&mut **tx)
    .await?;
    if legacy_collision {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO public.delivery_sent_bindings
         (world_id,external_mail_id,delivery_id,fencing_version,expectation_fencing_version)
         VALUES ($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING",
    )
    .bind(world_id)
    .bind(external_mail_id)
    .bind(delivery_id)
    .bind(fencing_version)
    .bind(has_expectation.then_some(fencing_version))
    .execute(&mut **tx)
    .await?;
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.delivery_sent_bindings
         WHERE world_id=$1 AND external_mail_id=$2 AND delivery_id=$3 AND fencing_version=$4
           AND expectation_fencing_version IS NOT DISTINCT FROM $5)",
    )
    .bind(world_id)
    .bind(external_mail_id)
    .bind(delivery_id)
    .bind(fencing_version)
    .bind(has_expectation.then_some(fencing_version))
    .fetch_one(&mut **tx)
    .await
}
