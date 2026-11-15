//! 在线持久开关入口；复用应用摘要和数据库事务，不调用停机或迁移。

use auction_application::runtime_switches::{
    RuntimeSwitchCommand, RuntimeSwitchName, RuntimeSwitchOperation, RuntimeSwitchStore,
};
use serde_json::{Value, json};

use crate::{app::print_json, cli::SwitchCommand, config_dir::ConfigDir, support::text};

pub(crate) async fn run(config_dir: &ConfigDir, command: SwitchCommand) -> anyhow::Result<()> {
    super::with_worker_store(
        config_dir,
        auction_storage::DatabaseConnectionPurpose::CliOperations,
        async |_, store| {
            let (name, value, operation, expires_at, write) = match command {
                SwitchCommand::List => {
                    let read = store.read().await?;
                    let switches: Vec<_> = RuntimeSwitchName::ALL
                        .into_iter()
                        .map(|name| {
                            let latest = read.events.iter().find(|event| event.name == name);
                            let revision = latest.map_or(0, |event| event.revision);
                            json!({"name": name, "revision": revision, "latest": latest})
                        })
                        .collect();
                    return print_json(
                        &json!({"database_time": read.database_time, "switches": switches}),
                    );
                }
                SwitchCommand::Set { name, value, write, expires_at, no_expiry: _ } => {
                    (name, Some(value), RuntimeSwitchOperation::Set, expires_at, write)
                }
                SwitchCommand::Clear { name, write } => {
                    (name, None, RuntimeSwitchOperation::Clear, None, write)
                }
            };
            let name = RuntimeSwitchName::parse(&name).ok_or_else(unknown_switch)?;
            let value = value.map(|value| switch_value(name, value)).transpose()?;
            let expires_at = expires_at
                .map(|value| {
                    chrono::DateTime::parse_from_rfc3339(&value)
                        .map(|time| time.with_timezone(&chrono::Utc))
                })
                .transpose()?;
            let event = store
                .write(RuntimeSwitchCommand {
                    command_id: text::command_id(write.command_id, "--command-id")?,
                    name,
                    expected_revision: write.expected_revision,
                    operation,
                    value,
                    expires_at,
                    expired_command_id: None,
                    actor_id: text::actor(&write.actor)?.to_owned(),
                    operator_identity: Some(super::operator_identity::capture()?),
                    entrypoint: "cli".to_owned(),
                    reason: write.reason,
                })
                .await?;
            print_json(&json!({
                "status": "database_accepted",
                "message": "数据库已接受；当前状态请核对最新事实与各角色指标",
                "accepted": event,
            }))
        },
    )
    .await
}

/// 名称闭集决定值的类型：两个口径开关是字符串（取值由应用命令校验），其余是布尔。
fn switch_value(name: RuntimeSwitchName, value: String) -> anyhow::Result<Value> {
    match name {
        RuntimeSwitchName::ReadinessAuditFailureMode | RuntimeSwitchName::DispositionReadMode => {
            Ok(json!(value))
        }
        RuntimeSwitchName::ReadinessCacheEnabled
        | RuntimeSwitchName::MetricsSliSnapshotEnabled
        | RuntimeSwitchName::MooglemailPollIsolationEnabled
        | RuntimeSwitchName::MetricsRejectForwardedRequests
        | RuntimeSwitchName::ExternalAccessPaused => {
            value.parse::<bool>().map(|value| json!(value)).map_err(invalid_boolean)
        }
    }
}

/// 未知名称时列出本版闭集，便于直接改正命令；不回显输入原文。
fn unknown_switch() -> anyhow::Error {
    let names = RuntimeSwitchName::ALL.map(RuntimeSwitchName::label);
    anyhow::anyhow!("未知开关名；可用名称：{}", names.join("、"))
}

fn invalid_boolean(_: std::str::ParseBoolError) -> anyhow::Error {
    anyhow::anyhow!("布尔开关只接受 true 或 false")
}

#[cfg(test)]
mod tests {
    use auction_application::runtime_switches::RuntimeSwitchName;

    use super::{switch_value, unknown_switch};

    #[test]
    fn values_follow_the_switch_type() {
        assert_eq!(
            switch_value(RuntimeSwitchName::ExternalAccessPaused, "true".to_owned()).expect("布尔"),
            serde_json::json!(true)
        );
        assert!(switch_value(RuntimeSwitchName::ExternalAccessPaused, "yes".to_owned()).is_err());
        assert_eq!(
            switch_value(RuntimeSwitchName::DispositionReadMode, "shadow".to_owned())
                .expect("字符串"),
            serde_json::json!("shadow")
        );
    }

    #[test]
    fn unknown_switch_lists_every_known_name() {
        let text = unknown_switch().to_string();
        for name in RuntimeSwitchName::ALL {
            assert!(text.contains(name.label()), "{text}");
        }
    }
}
