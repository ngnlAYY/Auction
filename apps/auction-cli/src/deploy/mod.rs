//! `auction-cli deploy <制品目录>`：分体部署的一条命令编排。
//!
//! 顺序：制品静态校验（文件白名单、权限、摘要、Web payload 标记）→ 严格配置与 unit 绑定预检
//! → 时段门禁 → 迁移版本与演练回执 → 在途发送门禁 → 停机 → 停机备份并由新旧
//! CLI 各校验一次 → 保留旧程序 → 原子安装 → 迁移与顺序启动 → 运行程序摘要复核 → 部署回执。
//! 任一门禁失败都在停机前拒绝；安装后启动失败只停止本次启动的 unit，并把回退指引写入回执。

use std::time::Duration;

use anyhow::Context;
use chrono::Utc;

use crate::app::{print_json, progress};
use crate::cli::DeployArgs;
use crate::config_dir::ConfigDir;
use crate::receipt::{Action, ReceiptContext};
use crate::service::{self, Systemctl};

pub(crate) mod artifact;
mod drill;
mod execution;
mod gates;
mod install;
mod receipts;
mod rollback;
mod verification;
use self::execution::deploy_locked;
use self::verification::{require_artifact_cli, require_backup_dir};
pub(crate) use receipts::{artifact_projection, receipt_summary, record_failure};
pub(crate) use verification::{role_binary, running_roles, shared_service_uid, unit_states};

pub(crate) use rollback::run as run_rollback;

pub(crate) async fn run(config_dir: &ConfigDir, args: DeployArgs) -> anyhow::Result<()> {
    let DeployArgs {
        artifact_dir,
        units,
        owner,
        shutdown,
        receipt: receipt_args,
        install_dir: explicit_install_dir,
        backup_dir,
        migration_drill_receipt,
        drill_max_age_days,
        send_gate_timeout_seconds,
        startup_timeout_seconds,
        dry_run,
    } = args;
    let artifact = artifact::inspect(&artifact_dir).context("部署制品校验失败")?;
    require_artifact_cli(&artifact)?;
    let mut context = ReceiptContext::resolve(None, receipt_args.actor.as_deref())?;
    let started_at = Utc::now();
    let owner = crate::database::load_owner_secrets(&owner.owner_secrets)?;
    let systemctl = Systemctl::new(&units)?;
    let _lock = crate::service::deployment_lock(&units)?;
    let deployment = service::prepare(config_dir, &systemctl).await?;
    let states = unit_states(&systemctl).await?;
    let running = running_roles(&states);
    let install_dir = gates::resolve_install_dir(explicit_install_dir.as_deref(), &running)?;
    install::require_install_dir(&install_dir)?;
    let service_uid = shared_service_uid(&states)?;

    let gate =
        service::shutdown_gate(config_dir, Some(&deployment.worker), "deploy", &shutdown).await?;
    let database = gates::database_name(&deployment.worker)?;
    let expected_version = auction_storage::postgres_expected_migration_version();
    let current_version = gates::database_version(&deployment.worker).await?;
    if expected_version < current_version {
        anyhow::bail!(
            "制品内嵌 schema 版本 {expected_version} 低于数据库 {current_version}；\
             回退请使用 auction-cli rollback，含破坏性迁移只能按 ops/deploy/RECOVERY.md 恢复"
        );
    }
    // 旧程序此刻正在服务版本 current_version 的数据库，因此它的有效基线就是当前版本；
    // 不额外调用旧 CLI（新配置可能含旧程序不认识的字段），也不把未证实的版本写进回滚记录。
    let previous_version = current_version;
    let drill = if expected_version > current_version {
        let path = migration_drill_receipt.as_deref().with_context(|| {
            format!(
                "本次制品包含迁移（数据库 {current_version} → {expected_version}），\
                 必须提供 --migration-drill-receipt"
            )
        })?;
        Some(
            drill::inspect(
                path,
                &drill::Requirements {
                    artifact: &artifact,
                    current_version,
                    expected_version,
                    protection_window_minutes: shutdown.protection_window_minutes,
                    max_age_days: drill_max_age_days,
                    now: Utc::now(),
                },
            )
            .await
            .context("迁移演练回执核对失败")?,
        )
    } else {
        None
    };
    progress(format_args!("在途发送门禁：最多等待 {send_gate_timeout_seconds} 秒"));
    let in_flight = gates::require_no_in_flight_sends(
        &deployment.worker,
        Duration::from_secs(send_gate_timeout_seconds),
    )
    .await?;

    let versions = serde_json::json!({
        "database": database,
        "current_version": current_version,
        "expected_version": expected_version,
        "previous_version": previous_version,
    });
    let drill_report = drill.as_ref().map(|drill| {
        serde_json::json!({
            "drill_id": drill.drill_id,
            "before_version": drill.before_version,
            "after_version": drill.after_version,
            "minimum_window_minutes": drill.minimum_window_minutes,
            "date_utc": drill.date_utc.to_string(),
        })
    });
    let mut report = serde_json::json!({
        "action": "deploy",
        "artifact": artifact_projection(&artifact),
        "install_directory": install_dir,
        "backup_directory": backup_dir,
        "gates": {
            "shutdown": gate,
            "versions": versions,
            "migration_drill": drill_report,
            "in_flight_sends": in_flight,
        },
    });
    if dry_run {
        report["outcome"] = "planned".into();
        report["dry_run"] = true.into();
        report["planned_steps"] = serde_json::json!([
            "按 Public → Admin → Worker 停止三角色",
            "创建停机备份并用新旧两个 auction-cli 各校验一次",
            "把当前程序保留到部署状态目录的回滚记录",
            "以原子替换安装制品程序",
            "用迁移 owner 执行迁移",
            "按 Worker → Admin → Public 启动并等待 readiness",
            "复核运行程序的摘要与制品一致",
            "追加部署回执",
        ]);
        return print_json(&report);
    }

    require_backup_dir(&backup_dir)?;
    let outcome = deploy_locked(
        config_dir,
        &units,
        &systemctl,
        &deployment,
        &owner,
        &artifact,
        &install_dir,
        &backup_dir,
        &database,
        previous_version,
        expected_version,
        drill.as_ref().map(|drill| drill.drill_id.as_str()),
        Duration::from_secs(startup_timeout_seconds),
        service_uid,
        &mut context,
        &gate,
        started_at,
    )
    .await;
    match outcome {
        Ok(value) => {
            report["outcome"] = "started".into();
            report["deployment"] = value;
            print_json(&report)
        }
        Err(error) => {
            record_failure(&units, &systemctl, Action::Deploy, started_at, &context, &error)
                .await?;
            Err(error)
        }
    }
}
