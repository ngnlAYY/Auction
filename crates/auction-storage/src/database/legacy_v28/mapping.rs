//! v28 预检错误和数据库标识辅助映射。

use super::*;

pub(super) fn query_error(stage: &'static str, source: sqlx::Error) -> PostgresLegacyV28Error {
    PostgresLegacyV28Error::Query { stage, source }
}

pub(super) fn validate_database_name(value: &str) -> Result<(), PostgresLegacyV28Error> {
    if !crate::database::valid_confirmed_database_name(value) {
        return Err(PostgresLegacyV28Error::InvalidDatabaseName);
    }
    Ok(())
}
