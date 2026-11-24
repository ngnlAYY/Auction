//! 为从未被拉黑的卖家写入测试用的解析中登记。

use auction_time::Timestamp;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

/// 不变量 1、3、6、8：登记投影、UUIDv4 账本回执和首条事件在同一个事务中写入。
pub(super) struct ResolvingRegistration<'a> {
    pub(super) listing_id: Uuid,
    pub(super) seller_user_id: Uuid,
    pub(super) eid: &'a str,
    pub(super) command_id: Uuid,
    pub(super) replay_key_id: Uuid,
    pub(super) request_digest: &'a [u8; 32],
    pub(super) created: Timestamp,
    pub(super) expires: Timestamp,
}

impl ResolvingRegistration<'_> {
    /// 写入固定为 main、非寄售、无拉黑历史的登记事实。
    pub(super) async fn insert(
        &self,
        tx: &mut Transaction<'_, Postgres>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO public.consignment_listings
             (id, seller_user_id, world_id, external_equipment_id, mode, state, price,
              pricing_version, fee_bps, state_version, service_epoch, blacklist_count_at_register,
              blacklist_baseline_ids, created_at, updated_at, expires_at)
             VALUES ($1, $2, 'main', $3, 'direct', 'resolving', 101, 1, 0, 1, 1, 0,
                     ARRAY[]::uuid[], $4, $4, $5)",
        )
        .bind(self.listing_id)
        .bind(self.seller_user_id)
        .bind(self.eid)
        .bind(self.created)
        .bind(self.expires)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            "INSERT INTO public.consignment_commands
             (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
              replay_key_id, listing_id, result_status, result_code, result_body, created_at)
             VALUES ($1, 'public_user', $2::uuid::text, 'register', 'new', $3, $4, $5, 202, 'accepted',
                     jsonb_build_object('command_id', $1::uuid, 'operation', 'register',
                         'listing_id', $5::uuid, 'listing_version', 1, 'state', 'resolving',
                         'disposition', 'registered', 'price', 101), $6)",
        )
        .bind(self.command_id)
        .bind(self.seller_user_id)
        .bind(self.request_digest.as_slice())
        .bind(self.replay_key_id)
        .bind(self.listing_id)
        .bind(self.created)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            "INSERT INTO public.consignment_events
             (listing_id, state_version, command_id, actor_kind, event_kind, from_state, to_state,
              reason_code, price, occurred_at)
             VALUES ($1, 1, $2, 'public_user', 'registered', NULL, 'resolving', NULL, 101, $3)",
        )
        .bind(self.listing_id)
        .bind(self.command_id)
        .bind(self.created)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }
}
