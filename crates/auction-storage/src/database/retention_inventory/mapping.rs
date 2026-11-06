//! retention 盘点的行映射、标识校验和摘要辅助。

use crate::database::retention_inventory::RetentionInventoryError;
use sha2::{Digest, Sha256};
use sqlx::{Row, postgres::PgRow};

pub(super) fn identity_string(
    row: &PgRow,
    column: &'static str,
    stage: &'static str,
) -> Result<String, RetentionInventoryError> {
    row.try_get(column).map_err(|source| query_error(stage, source))
}

pub(super) fn query_error(stage: &'static str, source: sqlx::Error) -> RetentionInventoryError {
    RetentionInventoryError::Query { stage, source }
}

pub(super) fn validate_database_name(value: &str) -> Result<(), RetentionInventoryError> {
    if !crate::database::valid_confirmed_database_name(value) {
        return Err(RetentionInventoryError::InvalidDatabaseName);
    }
    Ok(())
}

pub(super) fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

pub(super) fn sha256_hex(value: &[u8]) -> String {
    auction_application::core::lower_hex(&Sha256::digest(value))
}
