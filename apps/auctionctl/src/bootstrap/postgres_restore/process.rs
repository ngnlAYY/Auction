use super::*;

use std::process::{Command as ProcessCommand, Stdio};

pub(super) fn run_pg_restore(
    environment: &PostgresCommandEnvironment,
    confirmed_database_name: &str,
    archive_path: &Path,
) -> anyhow::Result<()> {
    // pg_restore 的数据库名使用只含 dbname 的安全 conninfo，命令行不含凭据。
    let database_conninfo =
        format!("dbname='{}'", confirmed_database_name.replace('\\', "\\\\").replace('\'', "\\'"));
    let mut command = ProcessCommand::new("pg_restore");
    command
        .args([
            "--no-password",
            "--exit-on-error",
            "--single-transaction",
            "--no-owner",
            "--no-privileges",
            "--dbname",
        ])
        .arg(database_conninfo)
        .arg(archive_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    environment.apply(&mut command);
    run_external_command(&mut command, "pg_restore")
        .context("PostgreSQL restore failed; single transaction should have rolled back")
}
