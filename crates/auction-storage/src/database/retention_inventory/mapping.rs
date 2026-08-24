//! retention 盘点的行映射、标识校验和摘要辅助。

use super::*;
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
    if value.is_empty()
        || value.trim() != value
        || value.len() > 63
        || value.chars().any(char::is_control)
    {
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
    Sha256::digest(value).iter().map(|byte| format!("{byte:02x}")).collect()
}
