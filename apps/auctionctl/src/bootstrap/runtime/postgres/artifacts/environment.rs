//! PostgreSQL 外部工具的连接环境与临时 passfile 管理。

use super::super::*;

pub(crate) struct PostgresCommandEnvironment {
    variables: Vec<(String, String)>,
    passfile_root: Option<PathBuf>,
    passfile: Option<PathBuf>,
}

impl PostgresCommandEnvironment {
    pub(crate) fn from_database_url(
        database_url: &str,
        confirmed_database_name: &str,
    ) -> anyhow::Result<Self> {
        let url = Url::parse(database_url).context("parse PostgreSQL database_url for libpq")?;
        if !matches!(url.scheme(), "postgres" | "postgresql")
            || url.fragment().is_some()
            || url.username().is_empty()
        {
            anyhow::bail!(
                "database_url is outside the secure PostgreSQL command connection profile"
            );
        }
        let raw_database = url.path().strip_prefix('/').unwrap_or(url.path());
        let database = decode_uri_component(raw_database)?;
        if database != confirmed_database_name || database.is_empty() || database.contains('/') {
            anyhow::bail!("database_url path does not match --confirm-database");
        }
        let username = decode_uri_component(url.username())?;
        let password = url.password().map(decode_uri_component).transpose()?;
        // SQLx 会从环境补齐省略的 host/port；外部 libpq 工具清理环境后必须仍使用同一目标。
        let host = url
            .host_str()
            .map(decode_uri_component)
            .transpose()?
            .context("PostgreSQL 备份/恢复 database_url 必须显式提供 host")?;
        // PGHOST 是 libpq 的 host 参数而非 URI authority；IPv6 去括号后同时用于 pgpass 匹配。
        let host = match url.host() {
            Some(url::Host::Ipv6(address)) => address.to_string(),
            _ => host,
        };
        let port = url
            .port()
            .context("PostgreSQL 备份/恢复 database_url 必须显式提供 port（默认端口也要写明）")?;
        if host.is_empty() || host.contains(',') {
            anyhow::bail!("database_url host is empty or uses an unsupported multi-host form");
        }

        let mut variables = vec![
            ("PGDATABASE".to_owned(), confirmed_database_name.to_owned()),
            ("PGUSER".to_owned(), username.clone()),
            ("PGHOST".to_owned(), host.clone()),
            ("PGPORT".to_owned(), port.to_string()),
            // 外部 PostgreSQL 工具不得继承机器级的无限连接等待或非 UTF-8 客户端编码。
            ("PGCONNECT_TIMEOUT".to_owned(), "5".to_owned()),
            ("PGCLIENTENCODING".to_owned(), "UTF8".to_owned()),
        ];
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
                "application_name" => {
                    require_pg_value(&value, "application_name", 63)?;
                    variables.push(("PGAPPNAME".to_owned(), value));
                }
                "connect_timeout" => {
                    let timeout = value
                        .parse::<u16>()
                        .ok()
                        .filter(|timeout| (1..=300).contains(timeout))
                        .ok_or_else(|| anyhow::anyhow!("connect_timeout must be 1..=300"))?;
                    variables.retain(|(key, _)| key != "PGCONNECT_TIMEOUT");
                    variables.push(("PGCONNECT_TIMEOUT".to_owned(), timeout.to_string()));
                }
                "sslmode" => {
                    if !matches!(
                        value.as_str(),
                        "disable" | "allow" | "prefer" | "require" | "verify-ca" | "verify-full"
                    ) {
                        anyhow::bail!(
                            "database_url sslmode is not supported by the PostgreSQL command adapter"
                        );
                    }
                    variables.push(("PGSSLMODE".to_owned(), value));
                }
                "ssl" if value == "true" => {
                    variables.push(("PGSSLMODE".to_owned(), "require".to_owned()));
                }
                // 未知键同样来自敏感 URL，可能是错放的凭据片段，不能回显。
                _ => anyhow::bail!(
                    "database_url query parameter is not supported by the backup adapter"
                ),
            }
        }

        let (passfile_root, passfile) = if let Some(password) = password {
            let root = std::env::temp_dir().join(format!("auctionctl-pgpass-{}", Uuid::new_v4()));
            let passfile_result: anyhow::Result<PathBuf> = (|| {
                fs::create_dir(&root).with_context(|| {
                    format!("create private PostgreSQL passfile directory: {}", root.display())
                })?;
                #[cfg(unix)]
                fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                    .context("restrict PostgreSQL passfile directory")?;
                let passfile = root.join("pgpass");
                let mut options = fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                options.mode(0o600);
                let mut file = options.open(&passfile).with_context(|| {
                    format!("create private PostgreSQL passfile: {}", passfile.display())
                })?;
                let port_value = port.to_string();
                let line = format!(
                    "{}:{}:{}:{}:{}\n",
                    pgpass_escape(&host),
                    pgpass_escape(&port_value),
                    pgpass_escape(confirmed_database_name),
                    pgpass_escape(&username),
                    pgpass_escape(&password),
                );
                file.write_all(line.as_bytes()).context("write private PostgreSQL passfile")?;
                file.sync_all().context("sync private PostgreSQL passfile")?;
                Ok(passfile)
            })();
            match passfile_result {
                Ok(passfile) => (Some(root), Some(passfile)),
                Err(error) => {
                    let _ = fs::remove_file(root.join("pgpass"));
                    let _ = fs::remove_dir(&root);
                    return Err(error);
                }
            }
        } else {
            (None, None)
        };

        Ok(Self { variables, passfile_root, passfile })
    }

    pub(crate) fn apply(&self, command: &mut ProcessCommand) {
        const POSTGRES_ENVIRONMENT_KEYS: &[&str] = &[
            "PGAPPNAME",
            "PGCHANNELBINDING",
            "PGCLIENTENCODING",
            "PGCONNECT_TIMEOUT",
            "PGDATABASE",
            "PGGSSDELEGATION",
            "PGGSSENCMODE",
            "PGGSSLIB",
            // hostaddr 在 libpq 中优先于 host，不能把目标选择交给继承的机器环境。
            "PGHOST",
            "PGHOSTADDR",
            "PGKRBSRVNAME",
            "PGLOADBALANCEHOSTS",
            "PGMAXPROTOCOLVERSION",
            "PGMINPROTOCOLVERSION",
            "PGOPTIONS",
            "PGPASSWORD",
            "PGPASSFILE",
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
        for key in POSTGRES_ENVIRONMENT_KEYS {
            command.env_remove(key);
        }
        for (key, value) in &self.variables {
            command.env(key, value);
        }
        if let Some(passfile) = &self.passfile {
            command.env("PGPASSFILE", passfile);
        }
    }
}

impl Drop for PostgresCommandEnvironment {
    fn drop(&mut self) {
        if let Some(passfile) = &self.passfile {
            let _ = fs::remove_file(passfile);
        }
        if let Some(root) = &self.passfile_root {
            let _ = fs::remove_dir(root);
        }
    }
}

pub(crate) fn require_pg_value(
    value: &str,
    name: &str,
    maximum_bytes: usize,
) -> anyhow::Result<()> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > maximum_bytes
        || value.chars().any(char::is_control)
    {
        anyhow::bail!("PostgreSQL {name} is invalid");
    }
    Ok(())
}

pub(crate) fn decode_uri_component(value: &str) -> anyhow::Result<String> {
    let mut bytes = Vec::with_capacity(value.len());
    let raw = value.as_bytes();
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'%' {
            if index + 2 >= raw.len() {
                anyhow::bail!("database_url contains an incomplete percent escape");
            }
            let high = hex_value(raw[index + 1])?;
            let low = hex_value(raw[index + 2])?;
            bytes.push((high << 4) | low);
            index += 3;
        } else {
            bytes.push(raw[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8(bytes).context("database_url contains invalid UTF-8")?;
    if decoded.chars().any(char::is_control) {
        anyhow::bail!("database_url contains a control character");
    }
    Ok(decoded)
}

pub(crate) fn hex_value(value: u8) -> anyhow::Result<u8> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => anyhow::bail!("database_url contains an invalid percent escape"),
    }
}

pub(crate) fn pgpass_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace(':', "\\:")
}
