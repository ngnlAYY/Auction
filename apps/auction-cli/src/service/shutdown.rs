//! 显式 stop/restart 在发出首个停止请求前读取时段证据；查询失败默认拒绝，
//! force 例外只作用于此门禁。停止前先输出声明，后续停机或迁移失败也保留原因。

use auction_config::AppConfig;
use serde::Serialize;
use serde_json::Value;

use crate::app::{print_json, progress};
use crate::cli::ShutdownArgs;
use crate::deployment::ConfigDir;

#[derive(Serialize)]
pub(crate) struct ShutdownGateReport<'a> {
    outcome: &'static str,
    protection_window_minutes: u32,
    force: bool,
    reason: Option<&'a str>,
    snapshot: Option<Value>,
    query_error: Option<String>,
}

pub(crate) async fn check<'a>(
    config_dir: &ConfigDir,
    worker: Option<&AppConfig>,
    action: &str,
    args: &'a ShutdownArgs,
) -> anyhow::Result<ShutdownGateReport<'a>> {
    let (snapshot, protected, query_error) =
        match inspect(config_dir, worker, args.protection_window_minutes).await {
            Ok((protected, snapshot)) => (Some(snapshot), protected, None),
            Err(error) => (None, true, Some(error.to_string())),
        };
    let outcome = if args.force {
        "forced"
    } else if protected {
        "blocked"
    } else {
        "allowed"
    };
    let report = ShutdownGateReport {
        outcome,
        protection_window_minutes: args.protection_window_minutes,
        force: args.force,
        reason: args.reason.as_deref(),
        snapshot,
        query_error,
    };
    // 只读门禁不写数据库审计；stderr 进入 journal，stdout 保持单个最终 JSON 文档。
    progress(format_args!("{action} 停机时段门禁：{}", serde_json::to_string(&report)?));
    if outcome == "blocked" {
        print_json(&serde_json::json!({
            "action": action, "outcome": "blocked", "shutdown_gate": report,
        }))?;
        anyhow::bail!(
            "时段门禁拒绝停机：存在 Final、已过截止或保护窗口内的在拍 lot，或只读证据不可用；\
             详见 shutdown_gate，人工例外必须同时提供 --force --reason <原因>"
        );
    }
    Ok(report)
}

#[cfg(feature = "postgres")]
async fn inspect(
    config_dir: &ConfigDir,
    worker: Option<&AppConfig>,
    protection_window_minutes: u32,
) -> anyhow::Result<(bool, Value)> {
    use auction_config::ConfigError;
    use auction_storage::StorageError;

    let loaded;
    let worker = match worker {
        Some(worker) => worker,
        None => {
            loaded = config_dir.load_worker().map_err(|error| {
                // InvalidAddress 等 ConfigError 会回显配置值，不能格式化原错误或错误链。
                let kind = match error.downcast_ref::<ConfigError>() {
                    Some(
                        ConfigError::Read { .. }
                        | ConfigError::SecretRead { .. }
                        | ConfigError::MoogleMailKeyEnvRead { .. }
                        | ConfigError::RuntimeRead { .. },
                    ) => "config_read",
                    Some(
                        ConfigError::Parse { .. }
                        | ConfigError::SecretParse { .. }
                        | ConfigError::MoogleMailKeyEnvParse { .. }
                        | ConfigError::RuntimeParse { .. },
                    ) => "config_parse",
                    Some(_) => "config_invalid",
                    None => "config_load",
                };
                anyhow::anyhow!("停机门禁无法加载 Worker 数据库配置（kind={kind}）")
            })?;
            &loaded
        }
    };
    let url =
        worker.database_url().ok_or_else(|| anyhow::anyhow!("停机门禁缺少 Worker 数据库连接"))?;
    let store = auction_storage::DatabaseStore::connect(
        url,
        crate::database::profile::resolve(
            worker,
            auction_storage::DatabaseConnectionPurpose::CliServiceCheck,
        ),
    )
    .await
    .map_err(|error| {
        // connect 返回 StorageError；只保留变体分类，不读取连接串或底层驱动错误。
        let kind = match error {
            StorageError::UnsupportedScheme(_) => "unsupported_scheme",
            StorageError::InvalidPostgresUrl => "invalid_postgres_url",
            StorageError::Connection { .. } => "connection",
            StorageError::Migration { .. } => "migration",
            StorageError::MigrationSourceInvariant { .. } => "migration_source_invariant",
            StorageError::MigrationTarget { .. } => "migration_target",
            StorageError::UnknownDatabase { .. } => "unknown_database",
            StorageError::RuntimePrivileges { .. } => "runtime_privileges",
        };
        anyhow::anyhow!("停机门禁无法连接 PostgreSQL（kind={kind}）")
    })?;
    let auction_storage::DatabaseStore::Postgres(postgres) = &store;
    let result = postgres.collect_shutdown_window(protection_window_minutes).await;
    store.postgres_pool().close().await;
    let snapshot = result?;
    let protected = !snapshot.final_auctions.is_empty() || !snapshot.ending_lots.is_empty();
    Ok((protected, serde_json::to_value(snapshot)?))
}

#[cfg(not(feature = "postgres"))]
async fn inspect(
    _config_dir: &ConfigDir,
    _worker: Option<&AppConfig>,
    _protection_window_minutes: u32,
) -> anyhow::Result<(bool, Value)> {
    anyhow::bail!("停机时段门禁需要 postgres feature；当前程序无法读取数据库证据")
}
