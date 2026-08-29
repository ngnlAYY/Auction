//! PostgreSQL bootstrap 的安全 SQL 拼接。

use super::model::PostgresDatabaseBootstrapError;

pub(super) fn validate_database_name(value: &str) -> Result<(), PostgresDatabaseBootstrapError> {
    if value.is_empty()
        || value.len() > 63
        || value.trim() != value
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(PostgresDatabaseBootstrapError::InvalidDatabaseName);
    }
    if matches!(value, "postgres" | "template0" | "template1") {
        return Err(PostgresDatabaseBootstrapError::ProtectedDatabase);
    }
    Ok(())
}

pub(super) fn validate_owner(value: &str) -> Result<(), PostgresDatabaseBootstrapError> {
    if value.is_empty()
        || value.len() > 63
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(PostgresDatabaseBootstrapError::InvalidOwner);
    }
    Ok(())
}

pub(super) fn create_database_sql(database_name: &str, owner: &str) -> String {
    format!("CREATE DATABASE {} OWNER {}", quote_identifier(database_name), quote_identifier(owner),)
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}
