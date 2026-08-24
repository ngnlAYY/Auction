#[cfg(feature = "postgres")]
use super::*;

#[cfg(feature = "postgres")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RecoveryReturnMetadata {
    pub(super) obligation_id: String,
    pub(super) credits: i64,
    pub(super) cod: i64,
    pub(super) attachment_ids: Vec<String>,
}

#[cfg(feature = "postgres")]
#[derive(Debug)]
pub(super) struct RecoveryReturnRow {
    pub(super) recovery_id: Uuid,
    pub(super) return_mail_id: Uuid,
    pub(super) return_external_mail_id: String,
    pub(super) return_world_id: String,
    pub(super) return_metadata: Value,
    pub(super) return_received_at: DateTime<Utc>,
    pub(super) returned_obligation_id: Uuid,
    pub(super) returned_obligation_kind: String,
    pub(super) returned_carrier_credit: i64,
    pub(super) external_equipment_id: String,
    pub(super) recovery_world_id: String,
    pub(super) payment_obligation_id: Uuid,
    pub(super) equipment_obligation_id: Option<Uuid>,
    pub(super) state_version: i64,
    pub(super) created_at: DateTime<Utc>,
}

#[cfg(feature = "postgres")]
#[derive(Debug)]
pub(super) struct ExpectedRecoveryReturn {
    pub(super) kind: &'static str,
    pub(super) expected_credits: i64,
    pub(super) expected_cod: i64,
    pub(super) expected_attachment_ids: Vec<String>,
}

#[cfg(feature = "postgres")]
pub(super) struct RecoveryActionInsert<'a> {
    pub(super) recovery_id: Uuid,
    pub(super) kind: &'a str,
    pub(super) ordinal: i32,
    pub(super) source_mail_id: Option<Uuid>,
    pub(super) expected_credits: i64,
    pub(super) expected_cod: i64,
    pub(super) expected_attachment_ids: &'a [String],
    pub(super) now: DateTime<Utc>,
}
