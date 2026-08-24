use super::model::{LocaleProvider, PostgresRebuildError, TargetDatabaseDefinition};

pub(super) fn create_database_sql(
    database_name: &str,
    owner: &str,
    definition: &TargetDatabaseDefinition,
) -> Result<String, PostgresRebuildError> {
    let mut sql = format!(
        "CREATE DATABASE {} WITH OWNER = {} TEMPLATE = template0 ENCODING = {}",
        quote_identifier(database_name),
        quote_identifier(owner),
        quote_literal(&definition.encoding),
    );
    match definition.locale_provider {
        LocaleProvider::Builtin => {
            let locale = definition.provider_locale.as_deref().ok_or(
                PostgresRebuildError::UnsafePreflight {
                    reason: "builtin locale provider is missing its provider locale",
                },
            )?;
            sql.push_str(" LOCALE_PROVIDER = builtin BUILTIN_LOCALE = ");
            sql.push_str(&quote_literal(locale));
        }
        LocaleProvider::Libc => {
            sql.push_str(" LOCALE_PROVIDER = libc LC_COLLATE = ");
            sql.push_str(&quote_literal(&definition.lc_collate));
            sql.push_str(" LC_CTYPE = ");
            sql.push_str(&quote_literal(&definition.lc_ctype));
        }
        LocaleProvider::Icu => {
            let locale = definition.provider_locale.as_deref().ok_or(
                PostgresRebuildError::UnsafePreflight {
                    reason: "ICU locale provider is missing its provider locale",
                },
            )?;
            sql.push_str(" LOCALE_PROVIDER = icu ICU_LOCALE = ");
            sql.push_str(&quote_literal(locale));
            sql.push_str(" LC_COLLATE = ");
            sql.push_str(&quote_literal(&definition.lc_collate));
            sql.push_str(" LC_CTYPE = ");
            sql.push_str(&quote_literal(&definition.lc_ctype));
            if let Some(rules) = definition.icu_rules.as_deref().filter(|rules| !rules.is_empty()) {
                sql.push_str(" ICU_RULES = ");
                sql.push_str(&quote_literal(rules));
            }
        }
    }
    sql.push_str(" TABLESPACE = ");
    sql.push_str(&quote_identifier(&definition.tablespace));
    sql.push_str(" CONNECTION LIMIT = ");
    sql.push_str(&definition.connection_limit.to_string());
    Ok(sql)
}

pub(super) fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn quote_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

pub(super) fn validate_database_name(value: &str) -> Result<(), PostgresRebuildError> {
    if value.is_empty()
        || value.len() > 63
        || value.trim() != value
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(PostgresRebuildError::InvalidDatabaseName);
    }
    Ok(())
}
