//! 世界当前收件目标的锁定读取、预览版本推进与装备 key 落库；Admin 录入与 MoogleMail 收件共用。
//! 调用方必须已持有 world 行锁（锁序 world → target/auction）；错误分类留给各自 owner。

use auction_domain::EquipmentKeyEnvelope;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

/// 以 `FOR UPDATE OF target, auction` 锁定的收件目标快照。
pub(crate) struct IntakeTarget {
    pub(crate) auction_id: Uuid,
    pub(crate) auction_sequence: i64,
    pub(crate) auction_status: String,
    pub(crate) state_version: i64,
    pub(crate) preview_revision: i64,
    pub(crate) intake_opens_at: Option<DateTime<Utc>>,
    world_id: String,
    target_status: String,
}

impl IntakeTarget {
    /// 目标属于该 world、指向状态一致的 draft/preview 拍卖且版本有效；否则调用方失败关闭。
    pub(crate) fn is_consistent(&self, world_id: &str) -> bool {
        self.world_id == world_id
            && self.auction_sequence > 0
            && matches!(self.auction_status.as_str(), "draft" | "preview")
            && self.target_status == self.auction_status
            && self.state_version > 0
            && self.preview_revision >= 0
    }

    /// Preview 期间收件需要同事务物化预览条目。
    pub(crate) fn is_preview(&self) -> bool {
        self.auction_status == "preview"
    }

    /// 新增装备后的 `(preview_revision, state_version)`；溢出时返回 `None`。
    pub(crate) fn next_versions(&self) -> Option<(i64, i64)> {
        Some((self.preview_revision.checked_add(1)?, self.state_version.checked_add(1)?))
    }
}

/// 锁定 world 当前收件目标及其拍卖；没有目标时返回 `None`。
pub(crate) async fn lock(
    connection: &mut PgConnection,
    world_id: &str,
) -> Result<Option<IntakeTarget>, sqlx::Error> {
    let Some(row) = sqlx::query(
        r#"SELECT target.auction_id,
                  target.target_status,
                  auction.world_id,
                  auction.sequence,
                  auction.status,
                  auction.state_version,
                  auction.preview_revision,
                  auction.intake_opens_at
           FROM public.auction_intake_targets AS target
           INNER JOIN public.auctions AS auction ON auction.id = target.auction_id
           WHERE target.world_id = $1
           FOR UPDATE OF target, auction"#,
    )
    .bind(world_id)
    .fetch_optional(connection)
    .await?
    else {
        return Ok(None);
    };
    Ok(Some(IntakeTarget {
        auction_id: row.try_get("auction_id")?,
        auction_sequence: row.try_get("sequence")?,
        auction_status: row.try_get("status")?,
        state_version: row.try_get("state_version")?,
        preview_revision: row.try_get("preview_revision")?,
        intake_opens_at: row.try_get("intake_opens_at")?,
        world_id: row.try_get("world_id")?,
        target_status: row.try_get("target_status")?,
    }))
}

/// 以锁定时的状态、预览修订和状态版本做 CAS 推进；返回 `false` 表示拍卖已变化。
pub(crate) async fn advance(
    connection: &mut PgConnection,
    target: &IntakeTarget,
    (next_preview_revision, next_state_version): (i64, i64),
    now: DateTime<Utc>,
) -> Result<bool, sqlx::Error> {
    sqlx::query(
        r#"UPDATE public.auctions
           SET preview_revision = $1, state_version = $2,
               updated_at = GREATEST(created_at, updated_at, $3)
           WHERE id = $4
             AND status = $7
             AND preview_revision = $5
             AND state_version = $6
           RETURNING state_version"#,
    )
    .bind(next_preview_revision)
    .bind(next_state_version)
    .bind(now)
    .bind(target.auction_id)
    .bind(target.preview_revision)
    .bind(target.state_version)
    .bind(&target.auction_status)
    .fetch_optional(connection)
    .await
    .map(|row| row.is_some())
}

/// 写入装备 key 信封；与装备行、lot 行同事务提交。
pub(crate) async fn insert_equipment_secret(
    connection: &mut PgConnection,
    equipment_id: Uuid,
    envelope: &EquipmentKeyEnvelope,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"INSERT INTO public.equipment_secrets
            (equipment_id, ciphertext, nonce, encryption_key_id, fingerprint, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, $6)"#,
    )
    .bind(equipment_id)
    .bind(envelope.ciphertext())
    .bind(envelope.nonce())
    .bind(envelope.encryption_key_id())
    .bind(envelope.fingerprint())
    .bind(now)
    .execute(connection)
    .await
    .map(drop)
}
