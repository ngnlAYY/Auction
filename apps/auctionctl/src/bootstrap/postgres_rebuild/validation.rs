//! PostgreSQL rebuild 的执行授权、连接 URL 和确认参数校验。

use std::collections::HashSet;

use anyhow::Context;
use auction_storage::postgres_expected_migration_version;
use url::Url;

use super::super::{decode_uri_component, require_pg_value};
use super::model::{Authorization, AuthorizationReceipt, Mode, Options};

pub(super) fn validate_execution_options(options: &Options<'_>) -> anyhow::Result<()> {
    match (options.execute, options.receipt) {
        (false, Some(_)) => {
            anyhow::bail!("postgres-rebuild 只读演练不写 receipt，请移除 --receipt")
        }
        (true, None) => anyhow::bail!("实际 postgres-rebuild 必须提供 --receipt"),
        _ => {}
    }
    if options.execute {
        let expected = format!("REBUILD {}", options.confirmed_database_name);
        if options.confirm_rebuild != Some(expected.as_str()) {
            anyhow::bail!("实际 postgres-rebuild 的 --confirm-rebuild 必须逐字匹配目标数据库");
        }
    }
    if options.backup_manifest.is_some() == options.allow_unrecoverable_ephemeral {
        anyhow::bail!(
            "postgres-rebuild 必须且只能选择 --backup-manifest 或 --allow-unrecoverable-ephemeral"
        );
    }
    if matches!(options.mode, Mode::EmptyForRestore) && options.backup_manifest.is_none() {
        anyhow::bail!("empty-for-restore 模式必须提供 --backup-manifest");
    }
    if options.allow_unrecoverable_ephemeral {
        let expected = format!("EPHEMERAL {}", options.confirmed_database_name);
        if options.confirm_ephemeral != Some(expected.as_str()) {
            anyhow::bail!("一次性环境豁免的 --confirm-ephemeral 必须逐字匹配目标数据库");
        }
    } else if options.confirm_ephemeral.is_some() {
        anyhow::bail!("--confirm-ephemeral 只能与 --allow-unrecoverable-ephemeral 同时使用");
    }
    Ok(())
}

pub(super) fn load_authorization<'a>(options: &Options<'a>) -> anyhow::Result<Authorization<'a>> {
    if let Some(manifest_path) = options.backup_manifest {
        let verified = super::super::postgres_restore::verify_backup_pair(
            manifest_path,
            options.confirmed_database_name,
        )?;
        let expected_binary_version = postgres_expected_migration_version();
        if verified.manifest.schema_version > expected_binary_version {
            anyhow::bail!(
                "backup schema version {} is newer than this binary's PostgreSQL migration version {}",
                verified.manifest.schema_version,
                expected_binary_version,
            );
        }
        let receipt = AuthorizationReceipt {
            kind: "verified_backup",
            manifest_sha256: Some(verified.manifest_sha256.clone()),
            archive_file_name: Some(verified.manifest.archive_file_name.clone()),
            archive_sha256: Some(verified.manifest.archive_sha256.clone()),
            backup_schema_version: Some(verified.manifest.schema_version),
            backup_created_at: Some(verified.manifest.created_at),
            unrecoverable_ephemeral: false,
        };
        Ok(Authorization::Backup { manifest_path, verified, receipt })
    } else {
        Ok(Authorization::Ephemeral {
            receipt: AuthorizationReceipt {
                kind: "unrecoverable_ephemeral",
                manifest_sha256: None,
                archive_file_name: None,
                archive_sha256: None,
                backup_schema_version: None,
                backup_created_at: None,
                unrecoverable_ephemeral: true,
            },
        })
    }
}

pub(crate) fn resolve_maintenance_database_url(
    application_database_url: &str,
    configured_maintenance_database_url: Option<&str>,
    confirmed_database_name: &str,
) -> anyhow::Result<String> {
    let application = parse_secure_postgres_url(
        application_database_url,
        confirmed_database_name,
        "application database_url",
    )?;
    let maintenance = match configured_maintenance_database_url {
        Some(database_url) => {
            let maintenance =
                parse_secure_postgres_url(database_url, "postgres", "maintenance database_url")?;
            if application.host_str() != maintenance.host_str()
                || application.port().unwrap_or(5432) != maintenance.port().unwrap_or(5432)
                || postgres_tls_profile(&application) != postgres_tls_profile(&maintenance)
            {
                anyhow::bail!(
                    "maintenance database_url must use the application database endpoint and TLS profile"
                );
            }
            maintenance
        }
        None => {
            let mut maintenance = application;
            maintenance.set_path("/postgres");
            maintenance
        }
    };
    Ok(maintenance.into())
}

pub(crate) fn application_database_username(
    application_database_url: &str,
    confirmed_database_name: &str,
) -> anyhow::Result<String> {
    let application = parse_secure_postgres_url(
        application_database_url,
        confirmed_database_name,
        "application database_url",
    )?;
    let username = decode_uri_component(application.username())?;
    require_pg_value(&username, "application role", 63)?;
    Ok(username)
}

fn parse_secure_postgres_url(
    database_url: &str,
    expected_database_name: &str,
    label: &'static str,
) -> anyhow::Result<Url> {
    let url = Url::parse(database_url).with_context(|| format!("parse {label} for rebuild"))?;
    if !matches!(url.scheme(), "postgres" | "postgresql")
        || url.fragment().is_some()
        || url.username().is_empty()
    {
        anyhow::bail!("{label} is outside the secure PostgreSQL rebuild connection profile");
    }
    let raw_database = url.path().strip_prefix('/').unwrap_or(url.path());
    let database = decode_uri_component(raw_database)?;
    if database != expected_database_name || database.is_empty() || database.contains('/') {
        anyhow::bail!("{label} path does not match its required database");
    }
    if url.host_str().is_some_and(|host| host.is_empty() || host.contains(',')) {
        anyhow::bail!("database_url host is empty or uses an unsupported multi-host form");
    }

    let mut seen_query_keys = HashSet::new();
    for (key, value) in url.query_pairs() {
        let key = key.into_owned();
        let value = value.into_owned();
        if !seen_query_keys.insert(key.clone()) {
            anyhow::bail!("database_url contains a duplicate PostgreSQL query parameter");
        }
        if (key == "ssl" && seen_query_keys.contains("sslmode"))
            || (key == "sslmode" && seen_query_keys.contains("ssl"))
        {
            anyhow::bail!("database_url cannot combine ssl and sslmode");
        }
        match key.as_str() {
            "application_name" => require_pg_value(&value, "application_name", 63)?,
            "connect_timeout" => {
                value
                    .parse::<u16>()
                    .ok()
                    .filter(|timeout| (1..=300).contains(timeout))
                    .ok_or_else(|| anyhow::anyhow!("connect_timeout must be 1..=300"))?;
            }
            "sslmode"
                if matches!(
                    value.as_str(),
                    "disable" | "allow" | "prefer" | "require" | "verify-ca" | "verify-full"
                ) => {}
            "ssl" if value == "true" => {}
            _ => anyhow::bail!(
                "database_url query parameter is not supported by the rebuild adapter: {key}"
            ),
        }
    }
    Ok(url)
}

fn postgres_tls_profile(url: &Url) -> &str {
    url.query_pairs()
        .find_map(|(key, value)| match key.as_ref() {
            "sslmode" => Some(match value.as_ref() {
                "disable" => "disable",
                "allow" => "allow",
                "prefer" => "prefer",
                "require" => "require",
                "verify-ca" => "verify-ca",
                "verify-full" => "verify-full",
                _ => "invalid",
            }),
            "ssl" if value == "true" => Some("require"),
            _ => None,
        })
        .unwrap_or("prefer")
}
