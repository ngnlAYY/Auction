//! pg_dump/pg_restore 的连接环境：从配置的连接串推导 libpq 环境变量，口令只写入临时私有
//! passfile，绝不进入进程参数；继承的 PG* 环境全部清除，避免目标被机器环境改写。

use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::Context;
use tokio::process::Command;

use crate::database::decode_component;

/// libpq 会读取的全部连接相关环境变量；启动外部工具前逐一清除。
const INHERITED: &[&str] = &[
    "PGAPPNAME",
    "PGCHANNELBINDING",
    "PGCLIENTENCODING",
    "PGCONNECT_TIMEOUT",
    "PGDATABASE",
    "PGGSSDELEGATION",
    "PGGSSENCMODE",
    "PGGSSLIB",
    "PGHOST",
    "PGHOSTADDR",
    "PGKRBSRVNAME",
    "PGLOADBALANCEHOSTS",
    "PGMAXPROTOCOLVERSION",
    "PGMINPROTOCOLVERSION",
    "PGOPTIONS",
    "PGPASSFILE",
    "PGPASSWORD",
    "PGPORT",
    "PGREQUIREAUTH",
    "PGREQUIREPEER",
    "PGREQUIRESSL",
    "PGSERVICE",
    "PGSERVICEFILE",
    "PGSSLCERT",
    "PGSSLCERTMODE",
    "PGSSLCOMPRESSION",
    "PGSSLCRL",
    "PGSSLCRLDIR",
    "PGSSLKEY",
    "PGSSLMAXPROTOCOLVERSION",
    "PGSSLMINPROTOCOLVERSION",
    "PGSSLMODE",
    "PGSSLNEGOTIATION",
    "PGSSLROOTCERT",
    "PGSSLSNI",
    "PGSYSCONFDIR",
    "PGTARGETSESSIONATTRS",
    "PGUSER",
];

/// 清除 libpq 会读取的继承变量；外部工具的身份只来自显式设置的环境。
pub(super) fn clear_inherited(command: &mut Command) {
    for key in INHERITED {
        command.env_remove(key);
    }
}

pub(super) struct Libpq {
    variables: Vec<(&'static str, String)>,
    passfile: Option<Passfile>,
}

/// 进程结束（含提前返回）即删除的临时 passfile。
struct Passfile {
    directory: PathBuf,
    file: PathBuf,
}

impl Drop for Passfile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.file);
        let _ = fs::remove_dir(&self.directory);
    }
}

impl Libpq {
    /// 只接受与 SQLx 共有且外部工具语义一致的参数；未知参数可能是错放的凭据，拒绝且不回显。
    pub(super) fn from_url(
        url: &str,
        database: &str,
        mode: auction_config::DatabaseProfileMode,
        purpose: auction_storage::DatabaseConnectionPurpose,
    ) -> anyhow::Result<Self> {
        let invalid = || anyhow::anyhow!("数据库连接参数不适用于备份/恢复工具");
        let parsed = url::Url::parse(url).map_err(|_| invalid())?;
        if parsed.fragment().is_some() {
            return Err(invalid());
        }
        let options = auction_storage::postgres_connect_options(url).map_err(|_| invalid())?;
        if options.get_database() != Some(database) {
            anyhow::bail!("--database 与 Worker 配置中的数据库名不一致");
        }
        if database.contains('/') || database.chars().any(char::is_control) {
            return Err(invalid());
        }
        let mut variables = vec![
            ("PGDATABASE", database.to_owned()),
            ("PGPORT", options.get_port().to_string()),
            ("PGCONNECT_TIMEOUT", "10".to_owned()),
            ("PGCLIENTENCODING", "UTF8".to_owned()),
        ];
        if mode == auction_config::DatabaseProfileMode::Profiled {
            variables.push(("PGAPPNAME", purpose.application_name().to_owned()));
            variables.push(("PGOPTIONS", "-c statement_timeout=0".to_owned()));
        }
        let mut password = parsed.password().map(decode_component).transpose()?;
        for (key, value) in parsed.query_pairs() {
            match key.as_ref() {
                "host" | "hostaddr" | "port" | "dbname" | "user" | "statement-cache-capacity" => {}
                "password" => password = Some(value.into_owned()),
                "sslmode" | "ssl-mode" => {
                    let mode = value.to_ascii_lowercase();
                    if !matches!(
                        mode.as_str(),
                        "disable" | "allow" | "prefer" | "require" | "verify-ca" | "verify-full"
                    ) {
                        return Err(invalid());
                    }
                    variables.push(("PGSSLMODE", mode));
                }
                "sslrootcert" | "ssl-root-cert" | "ssl-ca" => {
                    variables.push(("PGSSLROOTCERT", value.into_owned()));
                }
                "sslcert" | "ssl-cert" => variables.push(("PGSSLCERT", value.into_owned())),
                "sslkey" | "ssl-key" => variables.push(("PGSSLKEY", value.into_owned())),
                "application_name" => {}
                _ => return Err(invalid()),
            }
        }
        // SQLx 的 hostaddr 按查询顺序覆盖 TCP host，且不会清除先前指定的 Unix socket。
        // 只传最终 PGHOST，不保留 libpq 独立的 PGHOSTADDR，连接目标与 TLS 主机名均跟随 SQLx。
        let host = if let Some(socket) = options.get_socket() {
            socket.to_str().context("数据库 socket 路径不是 UTF-8")?
        } else {
            options.get_host()
        };
        let username = options.get_username();
        if host.is_empty() || username.is_empty() || username.chars().any(char::is_control) {
            return Err(invalid());
        }
        variables.push(("PGHOST", host.to_owned()));
        variables.push(("PGUSER", username.to_owned()));
        let passfile = match password {
            Some(password) => Some(Passfile::create(database, username, &password)?),
            None => None,
        };
        Ok(Self { variables, passfile })
    }

    fn apply(&self, command: &mut Command) {
        clear_inherited(command);
        for (key, value) in &self.variables {
            command.env(key, value);
        }
        if let Some(passfile) = &self.passfile {
            command.env("PGPASSFILE", &passfile.file);
        }
    }

    /// `pg_dump --format=custom`，不导出 owner 与权限；目标库由 PGDATABASE 决定。
    pub(super) async fn dump(&self, output: &Path) -> anyhow::Result<()> {
        let mut command = Command::new("pg_dump");
        command
            .args(["--no-password", "--format=custom", "--no-owner", "--no-privileges", "--file"])
            .arg(output);
        self.apply(&mut command);
        run(command, "pg_dump").await
    }

    /// 单事务恢复：任一对象失败即整体回滚；只传入仅含 dbname 的 conninfo，不含凭据。
    pub(super) async fn restore(&self, database: &str, archive: &Path) -> anyhow::Result<()> {
        let conninfo = format!("dbname='{}'", database.replace('\\', "\\\\").replace('\'', "\\'"));
        let mut command = Command::new("pg_restore");
        command
            .args([
                "--no-password",
                "--exit-on-error",
                "--single-transaction",
                "--no-owner",
                "--no-privileges",
                "--dbname",
            ])
            .arg(conninfo)
            .arg(archive);
        self.apply(&mut command);
        run(command, "pg_restore").await
    }
}

/// 只读取 archive 目录、不连接数据库，证明 archive 结构可被 pg_restore 解析。
pub(super) async fn list_archive(archive: &Path) -> anyhow::Result<()> {
    let mut command = Command::new("pg_restore");
    command.args(["--list", "--no-password"]).arg(archive);
    for key in INHERITED {
        command.env_remove(key);
    }
    run(command, "pg_restore --list").await
}

async fn run(mut command: Command, label: &str) -> anyhow::Result<()> {
    let status = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .kill_on_drop(true)
        .status()
        .await
        .with_context(|| format!("无法启动 {label}；请确认 PostgreSQL 客户端工具在 PATH 中"))?;
    if !status.success() {
        anyhow::bail!("{label} 失败：{status}");
    }
    Ok(())
}

impl Passfile {
    fn create(database: &str, username: &str, password: &str) -> anyhow::Result<Self> {
        let directory = std::env::temp_dir().join(format!("auction-cli-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .context("创建临时 passfile 目录失败")?;
        let passfile = Self { file: directory.join("pgpass"), directory };
        // 主机与端口已由 PGHOST/PGPORT 固定，passfile 只按库名与账号匹配。
        let line = format!("*:*:{}:{}:{}\n", escape(database), escape(username), escape(password));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&passfile.file)
            .context("创建临时 passfile 失败")?;
        file.write_all(line.as_bytes()).context("写入临时 passfile 失败")?;
        Ok(passfile)
    }
}

fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace(':', "\\:")
}
