//! PostgreSQL 外部备份与恢复工具的受限进程适配。

use super::super::*;
use super::{PostgresCommandEnvironment, verify_private_regular_file};

pub(crate) fn run_pg_dump(
    environment: &PostgresCommandEnvironment,
    output: &Path,
) -> anyhow::Result<()> {
    let mut command = ProcessCommand::new("pg_dump");
    command
        .args(["--no-password", "--format=custom", "--no-owner", "--no-privileges"])
        .arg(format!("--file={}", output.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    environment.apply(&mut command);
    run_external_command(&mut command, "pg_dump")
}

pub(crate) fn verify_archive(path: &Path) -> anyhow::Result<()> {
    verify_private_regular_file(path, "backup archive")?;
    let mut command = ProcessCommand::new("pg_restore");
    command.args(["--list", "--no-password"]).arg(path).stdin(Stdio::null()).stdout(Stdio::null());
    run_external_command(&mut command, "pg_restore --list")
}

pub(crate) fn run_external_command(
    command: &mut ProcessCommand,
    label: &str,
) -> anyhow::Result<()> {
    let status = command.status().with_context(|| format!("start {label}"))?;
    if !status.success() {
        anyhow::bail!("{label} failed with status {status}");
    }
    Ok(())
}
