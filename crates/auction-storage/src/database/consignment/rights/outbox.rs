//! 专用密文 outbox：每冻结身份 10 分钟至多一封携带挑战码的通知，码只以密文落库。
//!
//! 不变量 4、5、7：outbox 行是持久工作项，投递与“结果未知不重发”归 7D-5；这里只写行。
//! 节流以冻结身份（世界 + 身份文本）为范围，`locator_guide` 与 `challenge` 共用同一额度，
//! 调用方已持有该身份的 advisory 锁，所以“读最近一封再写”在并发下仍只有一封。
//! `sealed_payload` 的布局是 `密文 ‖ 64 字节小写十六进制明文指纹`（指纹用于解封时的认证，
//! 端口类型 `SealedChallengeSecret` 需要它）；`nonce` 与 `encryption_key_id` 各占独立列。
//! 行里没有明文码、挑战 MAC 或定位符。

use auction_application::consignment::SealedChallengeSecret;
use auction_domain::consignment::rights::notice_allowed;
use auction_time::Timestamp;
use sqlx::PgConnection;
use uuid::Uuid;

use super::TxError;

/// 待写入的携带挑战码的通知。
pub(super) struct NoticeRow {
    pub(super) kind: &'static str,
    pub(super) challenge_id: Uuid,
    pub(super) right_id: Uuid,
    pub(super) world_id: &'static str,
    pub(super) frozen_identity: String,
    pub(super) sealed: SealedChallengeSecret,
}

/// 该冻结身份最近一封携带码的通知距今是否已满 10 分钟。
pub(super) async fn slot_free(
    connection: &mut PgConnection,
    world: &str,
    frozen_identity: &str,
    now: Timestamp,
) -> Result<bool, TxError> {
    let last: Option<Timestamp> = sqlx::query_scalar(
        "SELECT max(created_at) FROM public.consignment_right_notice_outbox
         WHERE world_id = $1 AND frozen_identity = $2
           AND notice_kind IN ('locator_guide', 'challenge')",
    )
    .bind(world)
    .bind(frozen_identity)
    .fetch_one(connection)
    .await?;
    Ok(notice_allowed(last, now))
}

/// 写 outbox 行；`None`（虚拟或被节流的目标）不写任何行，其余写入路径与真实目标共用。
pub(super) async fn enqueue(
    connection: &mut PgConnection,
    notice: Option<NoticeRow>,
    now: Timestamp,
) -> Result<(), TxError> {
    let Some(notice) = notice else {
        return Ok(());
    };
    let mut payload = notice.sealed.ciphertext().to_vec();
    payload.extend_from_slice(notice.sealed.fingerprint().as_bytes());
    sqlx::query(
        "INSERT INTO public.consignment_right_notice_outbox
            (work_id, notice_kind, challenge_id, right_id, frozen_identity, world_id,
             sealed_payload, nonce, encryption_key_id, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(Uuid::new_v4())
    .bind(notice.kind)
    .bind(notice.challenge_id)
    .bind(notice.right_id)
    .bind(notice.frozen_identity)
    .bind(notice.world_id)
    .bind(payload)
    .bind(notice.sealed.nonce().as_slice())
    .bind(notice.sealed.encryption_key_id())
    .bind(now)
    .execute(connection)
    .await?;
    Ok(())
}
