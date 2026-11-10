//! Recovery 锁定范围和数据库行模型。

use {
    chrono::{DateTime, Utc},
    uuid::Uuid,
};

#[derive(Debug, Clone)]
pub(in crate::database::recovery::commands) struct RecoveryScope {
    pub(in crate::database::recovery::commands) auction_id: Uuid,
    pub(in crate::database::recovery::commands) lot_id: Uuid,
    pub(in crate::database::recovery::commands) world_id: String,
    pub(in crate::database::recovery::commands) equipment_id: Uuid,
}

#[derive(Debug, Clone)]
pub(in crate::database::recovery::commands) struct RecoveryRow {
    pub(in crate::database::recovery::commands) id: Uuid,
    pub(in crate::database::recovery::commands) auction_id: Uuid,
    pub(in crate::database::recovery::commands) lot_id: Uuid,
    pub(in crate::database::recovery::commands) world_id: String,
    pub(in crate::database::recovery::commands) equipment_id: Uuid,
    pub(in crate::database::recovery::commands) payment_obligation_id: Uuid,
    pub(in crate::database::recovery::commands) equipment_obligation_id: Option<Uuid>,
    pub(in crate::database::recovery::commands) seller_payout_obligation_id: Uuid,
    pub(in crate::database::recovery::commands) buyer_refund_obligation_id: Uuid,
    pub(in crate::database::recovery::commands) stage: String,
    pub(in crate::database::recovery::commands) resolution: Option<String>,
    pub(in crate::database::recovery::commands) state_version: i64,
    pub(in crate::database::recovery::commands) blocking_code: Option<String>,
    pub(in crate::database::recovery::commands) manual_reconcile_delivery_id: Option<Uuid>,
    pub(in crate::database::recovery::commands) relisted_auction_id: Option<Uuid>,
    pub(in crate::database::recovery::commands) relisted_lot_id: Option<Uuid>,
    pub(in crate::database::recovery::commands) custody_proof_kind: Option<String>,
    pub(in crate::database::recovery::commands) custody_proof_reference: Option<String>,
    pub(in crate::database::recovery::commands) custody_proof_digest: Option<Vec<u8>>,
    pub(in crate::database::recovery::commands) custody_proof_observed_at: Option<DateTime<Utc>>,
    pub(in crate::database::recovery::commands) created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub(in crate::database::recovery::commands) struct OriginalLot {
    pub(in crate::database::recovery::commands) equipment_id: Uuid,
    pub(in crate::database::recovery::commands) submission_id: Option<Uuid>,
    pub(in crate::database::recovery::commands) status: String,
    pub(in crate::database::recovery::commands) start_price: i64,
    pub(in crate::database::recovery::commands) equipment_world_id: String,
    pub(in crate::database::recovery::commands) equipment_category: String,
}

#[derive(Debug, Clone)]
pub(in crate::database::recovery::commands) struct ObligationRow {
    pub(in crate::database::recovery::commands) id: Uuid,
    pub(in crate::database::recovery::commands) auction_id: Uuid,
    pub(in crate::database::recovery::commands) lot_id: Uuid,
    pub(in crate::database::recovery::commands) world_id: String,
    pub(in crate::database::recovery::commands) kind: String,
    pub(in crate::database::recovery::commands) state: String,
    pub(in crate::database::recovery::commands) equipment_id: Uuid,
    pub(in crate::database::recovery::commands) amount: i64,
    pub(in crate::database::recovery::commands) recipient_user_id: Option<Uuid>,
    pub(in crate::database::recovery::commands) recipient_game_identity: Option<String>,
}

#[derive(Debug, Clone)]
pub(in crate::database::recovery::commands) struct ActionRow {
    pub(in crate::database::recovery::commands) id: Uuid,
    pub(in crate::database::recovery::commands) kind: String,
    pub(in crate::database::recovery::commands) state: String,
    pub(in crate::database::recovery::commands) delivery_id: Option<Uuid>,
    pub(in crate::database::recovery::commands) proof_kind: Option<String>,
    pub(in crate::database::recovery::commands) proof_reference: Option<String>,
    pub(in crate::database::recovery::commands) proof_digest: Option<Vec<u8>>,
    pub(in crate::database::recovery::commands) proof_observed_at: Option<DateTime<Utc>>,
}

pub(in crate::database::recovery::commands) struct RelistResult {
    pub(in crate::database::recovery::commands) stage: &'static str,
    pub(in crate::database::recovery::commands) resolution: &'static str,
    pub(in crate::database::recovery::commands) auction_id: Option<Uuid>,
    pub(in crate::database::recovery::commands) lot_id: Option<Uuid>,
    /// 永久不能重挂时的阻断码；停在 `relist_pending` 并退出自动重试，交由 Admin 人工处理。
    pub(in crate::database::recovery::commands) blocking_code: Option<&'static str>,
}

pub(in crate::database::recovery::commands) struct AuctionLock {
    pub(in crate::database::recovery::commands) world_id: String,
    pub(in crate::database::recovery::commands) status: String,
}
