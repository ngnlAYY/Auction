//! Recovery 锁定范围和数据库行模型。

#[cfg(feature = "postgres")]
use super::*;

#[cfg(feature = "postgres")]
#[derive(Debug, Clone)]
pub(in crate::database::recovery_commands) struct RecoveryScope {
    pub(in crate::database::recovery_commands) auction_id: Uuid,
    pub(in crate::database::recovery_commands) lot_id: Uuid,
    pub(in crate::database::recovery_commands) world_id: String,
    pub(in crate::database::recovery_commands) equipment_id: Uuid,
}

#[cfg(feature = "postgres")]
#[derive(Debug, Clone)]
pub(in crate::database::recovery_commands) struct RecoveryRow {
    pub(in crate::database::recovery_commands) id: Uuid,
    pub(in crate::database::recovery_commands) auction_id: Uuid,
    pub(in crate::database::recovery_commands) lot_id: Uuid,
    pub(in crate::database::recovery_commands) world_id: String,
    pub(in crate::database::recovery_commands) equipment_id: Uuid,
    pub(in crate::database::recovery_commands) payment_obligation_id: Uuid,
    pub(in crate::database::recovery_commands) equipment_obligation_id: Option<Uuid>,
    pub(in crate::database::recovery_commands) seller_payout_obligation_id: Uuid,
    pub(in crate::database::recovery_commands) buyer_refund_obligation_id: Uuid,
    pub(in crate::database::recovery_commands) stage: String,
    pub(in crate::database::recovery_commands) resolution: Option<String>,
    pub(in crate::database::recovery_commands) state_version: i64,
    pub(in crate::database::recovery_commands) blocking_code: Option<String>,
    pub(in crate::database::recovery_commands) manual_reconcile_delivery_id: Option<Uuid>,
    pub(in crate::database::recovery_commands) relisted_auction_id: Option<Uuid>,
    pub(in crate::database::recovery_commands) relisted_lot_id: Option<Uuid>,
    pub(in crate::database::recovery_commands) custody_proof_kind: Option<String>,
    pub(in crate::database::recovery_commands) custody_proof_reference: Option<String>,
    pub(in crate::database::recovery_commands) custody_proof_digest: Option<Vec<u8>>,
    pub(in crate::database::recovery_commands) custody_proof_observed_at: Option<DateTime<Utc>>,
    pub(in crate::database::recovery_commands) created_at: DateTime<Utc>,
}

#[cfg(feature = "postgres")]
#[derive(Debug, Clone)]
pub(in crate::database::recovery_commands) struct OriginalLot {
    pub(in crate::database::recovery_commands) equipment_id: Uuid,
    pub(in crate::database::recovery_commands) submission_id: Option<Uuid>,
    pub(in crate::database::recovery_commands) status: String,
    pub(in crate::database::recovery_commands) start_price: i64,
    pub(in crate::database::recovery_commands) equipment_world_id: String,
}

#[cfg(feature = "postgres")]
#[derive(Debug, Clone)]
pub(in crate::database::recovery_commands) struct ObligationRow {
    pub(in crate::database::recovery_commands) id: Uuid,
    pub(in crate::database::recovery_commands) auction_id: Uuid,
    pub(in crate::database::recovery_commands) lot_id: Uuid,
    pub(in crate::database::recovery_commands) world_id: String,
    pub(in crate::database::recovery_commands) kind: String,
    pub(in crate::database::recovery_commands) state: String,
    pub(in crate::database::recovery_commands) equipment_id: Uuid,
    pub(in crate::database::recovery_commands) amount: i64,
    pub(in crate::database::recovery_commands) recipient_user_id: Option<Uuid>,
    pub(in crate::database::recovery_commands) recipient_game_identity: Option<String>,
}

#[cfg(feature = "postgres")]
#[derive(Debug, Clone)]
pub(in crate::database::recovery_commands) struct ActionRow {
    pub(in crate::database::recovery_commands) id: Uuid,
    pub(in crate::database::recovery_commands) kind: String,
    pub(in crate::database::recovery_commands) state: String,
    pub(in crate::database::recovery_commands) delivery_id: Option<Uuid>,
    pub(in crate::database::recovery_commands) proof_kind: Option<String>,
    pub(in crate::database::recovery_commands) proof_reference: Option<String>,
    pub(in crate::database::recovery_commands) proof_digest: Option<Vec<u8>>,
    pub(in crate::database::recovery_commands) proof_observed_at: Option<DateTime<Utc>>,
}

#[cfg(feature = "postgres")]
pub(in crate::database::recovery_commands) struct RelistResult {
    pub(in crate::database::recovery_commands) stage: &'static str,
    pub(in crate::database::recovery_commands) resolution: &'static str,
    pub(in crate::database::recovery_commands) auction_id: Option<Uuid>,
    pub(in crate::database::recovery_commands) lot_id: Option<Uuid>,
}

#[cfg(feature = "postgres")]
pub(in crate::database::recovery_commands) struct AuctionLock {
    pub(in crate::database::recovery_commands) world_id: String,
    pub(in crate::database::recovery_commands) status: String,
    #[allow(dead_code)]
    pub(in crate::database::recovery_commands) state_version: i64,
}
