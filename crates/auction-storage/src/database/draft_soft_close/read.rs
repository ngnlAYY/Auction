//! 同一事务的草稿事实读取；存在发布/未知证据时交内层规则失败关闭。

use auction_application::draft_soft_close::{DraftSoftCloseError, DraftSoftCloseSnapshot};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use super::Failure;

/// 与生命周期相同的 world → round → auction 锁顺序；锁后再读全部资格事实。
pub(super) async fn lock_target(
    transaction: &mut Transaction<'_, Postgres>,
    auction_id: Uuid,
) -> Result<(), Failure> {
    let world: Option<String> = sqlx::query_scalar(
        "SELECT world.id FROM public.worlds AS world
         JOIN public.auctions AS auction ON auction.world_id = world.id
         WHERE auction.id = $1 FOR UPDATE OF world",
    )
    .bind(auction_id)
    .fetch_optional(&mut **transaction)
    .await?;
    if world.is_none() {
        return Err(DraftSoftCloseError::NotFound.into());
    }
    sqlx::query("SELECT id FROM public.scheduled_rounds WHERE auction_id = $1 FOR UPDATE")
        .bind(auction_id)
        .fetch_all(&mut **transaction)
        .await?;
    sqlx::query("SELECT id FROM public.auctions WHERE id = $1 FOR UPDATE")
        .bind(auction_id)
        .fetch_one(&mut **transaction)
        .await?;
    Ok(())
}

pub(super) async fn snapshot(
    transaction: &mut Transaction<'_, Postgres>,
    auction_id: Uuid,
) -> Result<DraftSoftCloseSnapshot, Failure> {
    let row = sqlx::query(
        "SELECT a.id, a.state_version, a.status, a.soft_close_window_seconds,
                a.soft_close_extension_seconds, a.default_start_price,
                a.proxy_slow_interval_seconds, a.policy_snapshot,
                (a.preview_opened_at IS NOT NULL OR a.preview_manual_open
                 OR a.opened_at IS NOT NULL OR a.final_phase_at IS NOT NULL
                 OR a.settled_at IS NOT NULL OR a.cancelled_at IS NOT NULL
                 OR a.scheduled_end_at IS NOT NULL OR a.effective_end_at IS NOT NULL)
                    AS lifecycle_evidence,
                (EXISTS (SELECT 1 FROM public.auction_forum_sync WHERE auction_id = a.id)
                 OR EXISTS (SELECT 1 FROM public.auction_forum_catalog_pages WHERE auction_id = a.id)
                 OR EXISTS (SELECT 1 FROM public.forum_sync_admin_commands WHERE auction_id = a.id)
                 OR EXISTS (SELECT 1 FROM public.auction_forum_bid_commands WHERE auction_id = a.id)
                 OR EXISTS (SELECT 1 FROM public.auction_preview_items WHERE auction_id = a.id)
                 OR EXISTS (SELECT 1 FROM public.auction_commands
                            WHERE aggregate_id = a.id AND command_kind IN
                              ('auction_preview', 'auction_open', 'auction_cancel',
                               'auction_final_phase', 'auction_early_settlement'))
                 OR EXISTS (SELECT 1 FROM public.audit_records
                            WHERE entity_id = a.id::text
                              AND action IN ('auction.preview_open', 'auction.open',
                                  'auction.cancel', 'auction.enter_final_phase',
                                  'auction.early_settlement'))
                 OR EXISTS (SELECT 1 FROM public.realtime_events
                            WHERE stream_key = 'auction:' || a.id::text
                              AND kind IN ('auction.preview_updated', 'auction.opened',
                                  'auction.cancelled', 'auction.final_phase_entered',
                                  'auction.early_settlement_requested')))
                    AS publication_evidence,
                (EXISTS (SELECT 1 FROM public.lots AS l
                         WHERE l.auction_id = a.id
                           AND (l.status NOT IN ('pending', 'withdrawn')
                                OR l.current_price IS NOT NULL OR l.leader_user_id IS NOT NULL
                                OR l.leader_proxy_last OR l.slow_authorization_id IS NOT NULL
                                OR l.slow_due_at IS NOT NULL
                                OR l.effective_end_at IS NOT NULL OR l.slow_generation <> 0
                                OR l.last_proxy_sequence <> 0))
                 OR EXISTS (SELECT 1 FROM public.bids AS b
                            JOIN public.lots AS l ON l.id = b.lot_id WHERE l.auction_id = a.id)
                 OR EXISTS (SELECT 1 FROM public.bid_proxy_authorizations AS p
                            JOIN public.lots AS l ON l.id = p.lot_id WHERE l.auction_id = a.id))
                    AS competition_evidence,
                (a.settlement_workflow_id IS NOT NULL OR a.settlement_claim_command_id IS NOT NULL
                 OR a.settlement_complete_command_id IS NOT NULL OR a.settlement_due_at IS NOT NULL
                 OR a.settlement_started_at IS NOT NULL OR a.settlement_attempt_count <> 0
                 OR a.settlement_next_attempt_at IS NOT NULL OR a.settlement_last_error_code IS NOT NULL
                 OR EXISTS (SELECT 1 FROM public.auction_settlements WHERE auction_id = a.id)
                 OR EXISTS (SELECT 1 FROM public.settlement_obligations WHERE auction_id = a.id)
                 OR EXISTS (SELECT 1 FROM public.lot_settlements AS s
                            JOIN public.lots AS l ON l.id = s.lot_id WHERE l.auction_id = a.id))
                    AS settlement_evidence,
                (NOT EXISTS (SELECT 1 FROM public.scheduled_rounds WHERE auction_id = a.id)
                 OR EXISTS (SELECT 1 FROM public.scheduled_rounds WHERE auction_id = a.id
                            AND (status <> 'active' OR preview_command_id IS NOT NULL
                                 OR open_command_id IS NOT NULL OR preview_completed_at IS NOT NULL
                                 OR open_completed_at IS NOT NULL OR final_completed_at IS NOT NULL
                                 OR final_command_id IS NOT NULL
                                 OR lease_owner IS NOT NULL OR lease_until IS NOT NULL)))
                    AS round_in_progress
         FROM public.auctions AS a WHERE a.id = $1",
    )
    .bind(auction_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(DraftSoftCloseError::NotFound)?;
    Ok(DraftSoftCloseSnapshot {
        auction_id: row.try_get("id")?,
        state_version: row.try_get("state_version")?,
        status: row.try_get("status")?,
        window_seconds: row.try_get("soft_close_window_seconds")?,
        extension_seconds: row.try_get("soft_close_extension_seconds")?,
        default_start_price: row.try_get("default_start_price")?,
        proxy_slow_interval_seconds: row.try_get("proxy_slow_interval_seconds")?,
        policy_snapshot: row.try_get("policy_snapshot")?,
        lifecycle_evidence: row.try_get("lifecycle_evidence")?,
        publication_evidence: row.try_get("publication_evidence")?,
        competition_evidence: row.try_get("competition_evidence")?,
        settlement_evidence: row.try_get("settlement_evidence")?,
        round_in_progress: row.try_get("round_in_progress")?,
    })
}
