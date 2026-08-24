//! PostgreSQL 备份、manifest、私有文件和 libpq 环境边界。

use super::*;

#[cfg(feature = "postgres")]
pub(crate) fn require_dump_extension(path: &Path) -> anyhow::Result<()> {
    if path.extension().and_then(|extension| extension.to_str()) != Some("dump") {
        anyhow::bail!("PostgreSQL backup output must use the .dump extension");
    }
    Ok(())
}

#[cfg(feature = "postgres")]
pub(crate) fn backup_manifest_path(archive_path: &Path) -> anyhow::Result<PathBuf> {
    let filename = archive_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("backup archive filename is not valid UTF-8"))?;
    Ok(archive_path.with_file_name(format!("{filename}.manifest")))
}

#[cfg(feature = "postgres")]
pub(crate) fn require_absent_artifact(path: &Path, label: &str) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => anyhow::bail!("{label} already exists; refusing to overwrite"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspect {label}: {}", path.display())),
    }
}

#[cfg(feature = "postgres")]
pub(crate) fn create_private_temp_path(target: &Path, label: &str) -> anyhow::Result<PathBuf> {
    let parent =
        target.parent().ok_or_else(|| anyhow::anyhow!("private artifact parent is missing"))?;
    for _ in 0..8 {
        let candidate = parent.join(format!(".{label}-{}.tmp", Uuid::new_v4()));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        match options.open(&candidate) {
            Ok(_) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("create private temporary artifact: {}", candidate.display())
                });
            }
        }
    }
    anyhow::bail!("could not allocate a private temporary backup artifact")
}

#[cfg(feature = "postgres")]
pub(crate) fn publish_private_artifact(temporary: &Path, target: &Path) -> anyhow::Result<()> {
    require_absent_artifact(target, "backup archive")?;
    fs::hard_link(temporary, target)
        .with_context(|| format!("publish backup archive: {}", target.display()))?;
    fs::remove_file(temporary)
        .with_context(|| format!("remove backup temporary artifact: {}", temporary.display()))?;
    Ok(())
}

#[cfg(feature = "postgres")]
pub(crate) fn remove_owned_temp(path: &Path) {
    let _ = fs::remove_file(path);
}

#[cfg(feature = "postgres")]
pub(crate) fn verify_private_regular_file(path: &Path, label: &str) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("read {label} metadata: {}", path.display()))?;
    if !metadata.file_type().is_file() {
        anyhow::bail!("{label} is not a regular non-symlink file: {}", path.display());
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o077 != 0 {
        anyhow::bail!("{label} allows group/other access: {}", path.display());
    }
    if metadata.len() == 0 {
        anyhow::bail!("{label} is empty: {}", path.display());
    }
    Ok(())
}

#[cfg(feature = "postgres")]
pub(crate) fn read_backup_manifest(path: &Path) -> anyhow::Result<PostgresBackupManifest> {
    verify_private_regular_file(path, "backup manifest")?;
    let metadata = fs::metadata(path)?;
    if metadata.len() > MAX_BACKUP_MANIFEST_BYTES {
        anyhow::bail!("backup manifest exceeds the bounded evidence size");
    }
    let bytes =
        fs::read(path).with_context(|| format!("read backup manifest: {}", path.display()))?;
    serde_json::from_slice(&bytes).context("decode PostgreSQL backup manifest")
}

#[cfg(feature = "postgres")]
pub(crate) fn validate_backup_manifest(
    manifest: &PostgresBackupManifest,
    confirmed_database_name: &str,
) -> anyhow::Result<()> {
    if manifest.format != POSTGRES_BACKUP_FORMAT
        || manifest.schema_version <= 0
        || manifest.archive_file_name.is_empty()
        || manifest.archive_file_name != manifest.archive_file_name.trim()
        || manifest.archive_file_name.contains('/')
        || manifest.archive_file_name.contains('\\')
        || manifest.archive_file_name.chars().any(char::is_control)
        || manifest.archive_file_name.ends_with(".manifest")
        || !manifest.archive_file_name.ends_with(".dump")
        || !is_sha256_hex(&manifest.database_name_sha256)
        || !is_sha256_hex(&manifest.archive_sha256)
    {
        anyhow::bail!("backup manifest identity or path fields are invalid");
    }
    if manifest.database_name_sha256 != sha256_text(confirmed_database_name) {
        anyhow::bail!("confirmed database name does not match backup manifest");
    }
    Ok(())
}

#[cfg(feature = "postgres")]
pub(crate) fn manifest_archive_path(
    manifest_path: &Path,
    manifest: &PostgresBackupManifest,
) -> anyhow::Result<PathBuf> {
    let manifest_filename = manifest_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("backup manifest filename is not valid UTF-8"))?;
    let expected_manifest_filename = format!("{}.manifest", manifest.archive_file_name);
    if manifest_filename != expected_manifest_filename {
        anyhow::bail!("backup manifest filename is not paired with its archive");
    }
    let parent = manifest_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("backup manifest parent is missing"))?;
    Ok(parent.join(&manifest.archive_file_name))
}

#[cfg(feature = "postgres")]
pub(crate) fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut file =
        fs::File::open(path).with_context(|| format!("open artifact: {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let bytes_read = file.read(&mut buffer).context("read artifact for SHA-256")?;
        if bytes_read == 0 {
            break;
        }
        digest.update(&buffer[..bytes_read]);
    }
    Ok(hex_digest(digest.finalize().as_slice()))
}

#[cfg(feature = "postgres")]
pub(crate) fn sha256_text(value: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(value.as_bytes());
    hex_digest(digest.finalize().as_slice())
}

#[cfg(feature = "postgres")]
pub(crate) fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(feature = "postgres")]
pub(crate) fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(feature = "postgres")]
pub(crate) fn require_backup_source(
    preflight: &auction_storage::PostgresRecoveryPreflight,
) -> anyhow::Result<i64> {
    let _identity = preflight
        .schema_identity
        .as_ref()
        .filter(|identity| identity.application == "auction" && identity.baseline_version > 0)
        .ok_or_else(|| {
            anyhow::anyhow!("backup source does not have a valid Auction schema identity")
        })?;
    let migration = preflight
        .migration
        .as_ref()
        .filter(|migration| {
            migration.all_success && migration.current && migration.latest_version > 0
        })
        .ok_or_else(|| {
            anyhow::anyhow!("backup source migration history is not current and clean")
        })?;
    Ok(migration.latest_version)
}

#[cfg(feature = "postgres")]
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

#[cfg(feature = "postgres")]
pub(crate) fn verify_archive(path: &Path) -> anyhow::Result<()> {
    verify_private_regular_file(path, "backup archive")?;
    let mut command = ProcessCommand::new("pg_restore");
    command.args(["--list", "--no-password"]).arg(path).stdin(Stdio::null()).stdout(Stdio::null());
    run_external_command(&mut command, "pg_restore --list")
}

#[cfg(feature = "postgres")]
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

#[cfg(feature = "postgres")]
pub(crate) struct PostgresCommandEnvironment {
    variables: Vec<(String, String)>,
    passfile_root: Option<PathBuf>,
    passfile: Option<PathBuf>,
}

#[cfg(feature = "postgres")]
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
        let host = url.host_str().map(decode_uri_component).transpose()?;
        if host.as_deref().is_some_and(|host| host.is_empty() || host.contains(',')) {
            anyhow::bail!("database_url host is empty or uses an unsupported multi-host form");
        }

        let mut variables = vec![
            ("PGDATABASE".to_owned(), confirmed_database_name.to_owned()),
            ("PGUSER".to_owned(), username.clone()),
            // 外部 PostgreSQL 工具不得继承机器级的无限连接等待或非 UTF-8 客户端编码。
            ("PGCONNECT_TIMEOUT".to_owned(), "5".to_owned()),
            ("PGCLIENTENCODING".to_owned(), "UTF8".to_owned()),
        ];
        if let Some(host) = &host {
            variables.push(("PGHOST".to_owned(), host.clone()));
        }
        if let Some(port) = url.port() {
            variables.push(("PGPORT".to_owned(), port.to_string()));
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
                _ => anyhow::bail!(
                    "database_url query parameter is not supported by the backup adapter: {key}"
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
                let host_value = host.as_deref().unwrap_or("*");
                let port_value = url.port().map_or_else(|| "*".to_owned(), |port| port.to_string());
                let line = format!(
                    "{}:{}:{}:{}:{}\n",
                    pgpass_escape(host_value),
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

#[cfg(feature = "postgres")]
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

#[cfg(feature = "postgres")]
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

#[cfg(feature = "postgres")]
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

#[cfg(feature = "postgres")]
pub(crate) fn hex_value(value: u8) -> anyhow::Result<u8> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => anyhow::bail!("database_url contains an invalid percent escape"),
    }
}

#[cfg(feature = "postgres")]
pub(crate) fn pgpass_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace(':', "\\:")
}

#[cfg(feature = "postgres")]
pub(crate) fn validate_private_evidence_path(path: &Path) -> anyhow::Result<()> {
    if !path.is_absolute() || path == Path::new("/") {
        anyhow::bail!("private evidence output must be a non-root absolute path");
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| anyhow::anyhow!("private evidence output parent is missing"))?;
    let canonical_parent = fs::canonicalize(parent)
        .with_context(|| format!("canonicalize output parent: {}", parent.display()))?;
    if canonical_parent != parent {
        anyhow::bail!("private evidence output parent must be a canonical non-symlink path");
    }
    if path.file_name().is_none() {
        anyhow::bail!("private evidence output filename is missing");
    }
    Ok(())
}

#[cfg(feature = "postgres")]
pub(crate) fn write_private_evidence(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    validate_private_evidence_path(path)?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| anyhow::anyhow!("retention inventory output parent is missing"))?;
    let metadata = fs::symlink_metadata(parent)
        .with_context(|| format!("read retention inventory parent: {}", parent.display()))?;
    if !metadata.file_type().is_dir() {
        anyhow::bail!("retention inventory output parent is not a directory: {}", parent.display());
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o022 != 0 {
        anyhow::bail!(
            "retention inventory output parent is group/other writable: {}",
            parent.display()
        );
    }

    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .with_context(|| format!("create retention inventory evidence: {}", path.display()))?;
    file.write_all(contents).context("write retention inventory evidence")?;
    file.write_all(b"\n").context("terminate retention inventory evidence")?;
    file.sync_all().context("sync retention inventory evidence")?;
    Ok(())
}

#[cfg(feature = "postgres")]
pub(crate) fn read_bootstrap_cookie() -> anyhow::Result<String> {
    const MAX_COOKIE_BYTES: usize = 16 * 1024;
    let mut bytes = Vec::with_capacity(4096);
    std::io::stdin().take((MAX_COOKIE_BYTES + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_COOKIE_BYTES {
        anyhow::bail!("bootstrap mail Cookie 超过允许长度");
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    String::from_utf8(bytes).map_err(Into::into)
}
