use super::*;

use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub(super) struct PostgresRestoreReceipt {
    pub(super) format: &'static str,
    pub(super) restored_at: DateTime<Utc>,
    pub(super) database_name: String,
    pub(super) database_name_sha256: String,
    pub(super) archive_file_name: String,
    pub(super) archive_sha256: String,
    pub(super) manifest_sha256: String,
    pub(super) schema_version: i64,
    pub(super) single_transaction: bool,
    pub(super) ownership_restored: bool,
    pub(super) privileges_restored: bool,
}

pub(super) type RestoreReceiptReservation = PrivateReceiptReservation<RestoreReceiptSpec>;
