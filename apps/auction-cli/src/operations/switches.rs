//! 在线持久开关入口；复用应用摘要和数据库事务，不调用停机或迁移。

use crate::{app::print_json, cli::SwitchCommand, deployment::ConfigDir, support::text};
use auction_application::{
    RuntimeSwitchCommand, RuntimeSwitchName, RuntimeSwitchOperation, RuntimeSwitchStore,
};
use serde_json::json;

pub(crate) async fn run(config_dir: &ConfigDir, command: SwitchCommand) -> anyhow::Result<()> {
    let (_, store) =
        super::worker_store(config_dir, auction_storage::DatabaseConnectionPurpose::CliOperations)
            .await?;
    let result = async {
        let (name, value, operation, expires_at, write) = match command {
            SwitchCommand::List => {
                let read = store.read().await?;
                let switches: Vec<_> = RuntimeSwitchName::ALL.into_iter().map(|name| {
                    let latest = read.events.iter().find(|event| event.name == name);
                    json!({"name": name, "revision": latest.map_or(0, |event| event.revision), "latest": latest})
                }).collect();
                return print_json(&json!({"database_time": read.database_time, "switches": switches}));
            }
            SwitchCommand::Set { name, value, write, expires_at, no_expiry: _ } => (name, Some(value), RuntimeSwitchOperation::Set, expires_at, write),
            SwitchCommand::Clear { name, write } => (name, None, RuntimeSwitchOperation::Clear, None, write),
        };
        let name = RuntimeSwitchName::parse(&name).ok_or_else(|| anyhow::anyhow!("未知开关名"))?;
        let value = value.map(|value| match name {
            RuntimeSwitchName::ReadinessAuditFailureMode => Ok(json!(value)),
            RuntimeSwitchName::ReadinessCacheEnabled
            | RuntimeSwitchName::MetricsSliSnapshotEnabled
            | RuntimeSwitchName::MooglemailPollIsolationEnabled
            | RuntimeSwitchName::MetricsRejectForwardedRequests => value
                .parse::<bool>()
                .map(|value| json!(value))
                .map_err(invalid_boolean),
        }).transpose()?;
        let expires_at = expires_at.map(|value| chrono::DateTime::parse_from_rfc3339(&value).map(|time| time.with_timezone(&chrono::Utc))).transpose()?;
        let event = store.write(RuntimeSwitchCommand {
            command_id: text::command_id(write.command_id, "--command-id")?, name,
            expected_revision: write.expected_revision, operation, value, expires_at,
            expired_command_id: None, actor_id: text::actor(&write.actor)?.to_owned(),
            operator_identity: Some(super::operator_identity::capture()?),
            entrypoint: "cli".to_owned(), reason: write.reason,
        }).await?;
        print_json(&json!({"status": "database_accepted", "message": "数据库已接受；当前状态请核对最新事实与各角色指标", "accepted": event}))
    }.await;
    store.postgres_pool().close().await;
    result
}

fn invalid_boolean(_: std::str::ParseBoolError) -> anyhow::Error {
    anyhow::anyhow!("布尔开关只接受 true 或 false")
}
