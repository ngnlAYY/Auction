//! 每周物理基础备份（W10.7）：`pg_basebackup` 的 tar 流经 zstd 压缩与 age 公钥加密后
//! fsync、原子发布到暂存目录，由 `wal upload` 上传 R2 的 `base/` 前缀。
//!
//! 本命令按 PostgreSQL 服务账号执行：只读取数据库连接（默认 socket + peer 认证），
//! 不读敏感 TOML，也不持有 age 私钥。`-Ft -D -` 要求集群没有额外表空间，且不能与
//! `-X stream` 同用，因此使用 `-X fetch`：所需 WAL 会写进同一个 `base.tar` 流，
//! 产物是可直接用于时间点恢复的独立基础备份。

use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    time::SystemTime,
};

use anyhow::Context;
use chrono::Utc;
use serde::Serialize;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite},
    process::Command,
};

use super::model::{
    BASE_MANIFEST_FORMAT, BaseManifest, base_cipher_path, base_id, base_manifest_path,
    require_staging_dir, valid_recipient,
};
use crate::{
    app::progress,
    backup::{libpq, manifest::sha256_file},
};

/// 子进程 stderr 的有界捕获上限；失败原因进入 systemd 日志，不进入回执文件。
const STDERR_LIMIT: usize = 4096;
/// 暂存文件允许同组读取：基础备份账号（postgres）与上传账号可能不同。
const STAGED_MODE: u32 = 0o640;
/// manifest 只是本机证据，不承载备份内容；上限与 WAL 归档 manifest 一致。
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

/// 基础备份命令的输入；备份身份由开始时刻派生，不由调用方指定。
pub(super) struct Request {
    pub(super) staging_dir: PathBuf,
    pub(super) recipient: String,
}

/// 基础备份的机器可读结果。
#[derive(Debug, Serialize)]
pub(super) struct Outcome {
    pub(super) backup_id: String,
    pub(super) status: CreateStatus,
    pub(super) plaintext_bytes: u64,
    pub(super) cipher_bytes: u64,
    pub(super) cipher_sha256: String,
    pub(super) staging: PathBuf,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CreateStatus {
    Created,
    AlreadyCreated,
}

/// 校验输入后按同一秒的既有身份判定重复，再执行基础备份。
pub(super) async fn run(request: &Request) -> anyhow::Result<Outcome> {
    require_staging_dir(&request.staging_dir)?;
    if !valid_recipient(&request.recipient) {
        anyhow::bail!("age 收件人公钥无效；须与 [backup.offsite] age_recipient 相同");
    }
    let started = unix_seconds()?;
    let backup_id = base_id(Utc::now());
    if let Some(outcome) = resolve_existing(&request.staging_dir, &backup_id)? {
        return Ok(outcome);
    }
    let tools = Tools {
        source: Tool {
            program: OsString::from("pg_basebackup"),
            args: source_args(&backup_id),
            clear_pg_env: true,
            env: vec![
                ("PGDATABASE", "postgres"),
                ("PGAPPNAME", "auction-cli-base-backup"),
                ("PGCONNECT_TIMEOUT", "10"),
                ("PGCLIENTENCODING", "UTF8"),
            ],
        },
        compress: Tool {
            program: OsString::from("zstd"),
            args: vec![OsString::from("-q"), OsString::from("-T1"), OsString::from("-c")],
            clear_pg_env: false,
            env: Vec::new(),
        },
        encrypt: Tool {
            program: OsString::from("age"),
            args: vec![
                OsString::from("--encrypt"),
                OsString::from("--recipient"),
                OsString::from(request.recipient.as_str()),
            ],
            clear_pg_env: false,
            env: Vec::new(),
        },
    };
    publish(&tools, &request.staging_dir, &backup_id, started).await
}

/// `-D -` 只允许 tar 格式且不能使用 WAL streaming；`-X fetch` 把所需 WAL 并入 `base.tar`。
fn source_args(backup_id: &str) -> Vec<OsString> {
    vec![
        OsString::from("--pgdata"),
        OsString::from("-"),
        OsString::from("--format"),
        OsString::from("tar"),
        OsString::from("--wal-method"),
        OsString::from("fetch"),
        OsString::from("--checkpoint"),
        OsString::from("fast"),
        OsString::from("--label"),
        OsString::from(backup_id),
        OsString::from("--no-password"),
    ]
}

/// 同一秒内的重复运行沿用既有身份；只有密文而没有 manifest 表示上次中断，删除后重做。
fn resolve_existing(directory: &Path, backup_id: &str) -> anyhow::Result<Option<Outcome>> {
    let cipher = base_cipher_path(directory, backup_id);
    let manifest = base_manifest_path(directory, backup_id);
    let cipher_metadata = regular_or_none(&cipher)?;
    let manifest_metadata = regular_or_none(&manifest)?;
    match (cipher_metadata, manifest_metadata) {
        (None, None) => Ok(None),
        (Some(_), None) => {
            fs::remove_file(&cipher)
                .with_context(|| format!("清理未确认的基础备份密文失败：{}", cipher.display()))?;
            sync_directory(directory)?;
            progress(format_args!(
                "wal_base_backup status=discarded_unconfirmed backup_id={backup_id}"
            ));
            Ok(None)
        }
        (None, Some(_)) => {
            anyhow::bail!("基础备份 manifest 存在但密文缺失：{}", manifest.display())
        }
        (Some(metadata), Some(_)) => {
            let record = read_manifest(&manifest)?;
            record.validate(backup_id)?;
            if metadata.len() != record.cipher_bytes {
                anyhow::bail!("基础备份密文长度与 manifest 不一致：{}", cipher.display());
            }
            progress(format_args!("wal_base_backup status=already_created backup_id={backup_id}"));
            Ok(Some(Outcome {
                backup_id: backup_id.to_owned(),
                status: CreateStatus::AlreadyCreated,
                plaintext_bytes: record.plaintext_bytes,
                cipher_bytes: record.cipher_bytes,
                cipher_sha256: record.cipher_sha256,
                staging: cipher,
            }))
        }
    }
}

/// 一个外部过滤程序；测试注入 `/bin/cat` 或 `/bin/false` 验证发布与失败传播。
struct Tool {
    program: OsString,
    args: Vec<OsString>,
    /// 启动前清除继承的 libpq 环境；只有 `pg_basebackup` 需要。
    clear_pg_env: bool,
    env: Vec<(&'static str, &'static str)>,
}

struct Tools {
    source: Tool,
    compress: Tool,
    encrypt: Tool,
}

fn command(tool: &Tool) -> Command {
    let mut command = Command::new(&tool.program);
    command.args(&tool.args).kill_on_drop(true);
    if tool.clear_pg_env {
        libpq::clear_inherited(&mut command);
    }
    for (key, value) in &tool.env {
        command.env(key, value);
    }
    command
}

/// 写入临时文件，三个外部程序都成功后 fsync、改名并发布 manifest。
async fn publish(
    tools: &Tools,
    directory: &Path,
    backup_id: &str,
    started: u64,
) -> anyhow::Result<Outcome> {
    let temp = temp_path(directory, backup_id);
    remove_stale(&temp)?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&temp)
        .with_context(|| format!("创建基础备份密文临时文件失败：{}", temp.display()))?;
    let result = write_cipher(tools, directory, backup_id, file, started).await;
    if result.is_err() {
        // 只回收本次未发布的临时文件；已发布的密文由下次运行按 manifest 判定。
        discard(&temp);
    }
    result
}

async fn write_cipher(
    tools: &Tools,
    directory: &Path,
    backup_id: &str,
    file: File,
    started: u64,
) -> anyhow::Result<Outcome> {
    let mut source = command(&tools.source)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "无法启动 {}；请确认基础备份账号 PATH 与数据库连接",
                tools.source.program.to_string_lossy()
            )
        })?;
    let source_out = source.stdout.take().context("pg_basebackup 输出管道不可用")?;
    let source_errors = source.stderr.take().context("pg_basebackup 错误管道不可用")?;
    let mut compress = command(&tools.compress)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "无法启动 {}；请确认基础备份账号 PATH",
                tools.compress.program.to_string_lossy()
            )
        })?;
    let compress_in = compress.stdin.take().context("压缩输入管道不可用")?;
    let compress_out = compress.stdout.take().context("压缩输出管道不可用")?;
    let compress_errors = compress.stderr.take().context("压缩错误管道不可用")?;
    let mut encrypt = command(&tools.encrypt)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(file.try_clone().context("复制基础备份密文句柄失败")?))
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!("无法启动 {}；请确认基础备份账号 PATH", tools.encrypt.program.to_string_lossy())
        })?;
    let encrypt_in = encrypt.stdin.take().context("加密输入管道不可用")?;
    let encrypt_errors = encrypt.stderr.take().context("加密错误管道不可用")?;
    // 三段必须同时流动：只有一段的输入被转发时，中间进程写满自己的输出管道就会阻塞，
    // 进而停止读取输入，整条链死锁。每段一个转发任务并同时排空自己的 stderr；
    // 下游提前退出时上游收到 EPIPE，任务自然会结束。
    let source_task =
        tokio::spawn(
            async move { forward_and_capture(source_out, compress_in, source_errors).await },
        );
    let compress_task = tokio::spawn(async move {
        forward_and_capture(compress_out, encrypt_in, compress_errors).await
    });
    let (encrypt_status, encrypt_text) =
        tokio::join!(encrypt.wait(), capture(encrypt_errors, STDERR_LIMIT));
    let encrypt_status = encrypt_status.context("等待加密进程失败")?;
    let compress_status = compress.wait().await.context("等待压缩进程失败")?;
    let source_status = source.wait().await.context("等待 pg_basebackup 进程失败")?;
    let (plaintext, source_text) = source_task.await.context("等待第一个转发任务失败")?;
    let (compressed, compress_text) = compress_task.await.context("等待第二个转发任务失败")?;

    let mut failures = Vec::new();
    if !source_status.success() {
        failures.push(format!(
            "pg_basebackup 失败（{}）：{}",
            describe_status(source_status),
            describe(source_text)
        ));
    }
    if !compress_status.success() {
        failures.push(format!(
            "zstd 压缩失败（{}）：{}",
            describe_status(compress_status),
            describe(compress_text)
        ));
    }
    if !encrypt_status.success() {
        failures.push(format!(
            "age 加密失败（{}）：{}",
            describe_status(encrypt_status),
            describe(encrypt_text)
        ));
    }
    if !failures.is_empty() {
        anyhow::bail!("基础备份未完成：{}", failures.join("；"));
    }
    let plaintext = plaintext.context("转发 pg_basebackup 输出到 zstd 失败")?;
    compressed.context("转发压缩输出到 age 失败")?;
    if plaintext == 0 {
        anyhow::bail!("pg_basebackup 没有输出任何 tar 字节");
    }
    // 用文件描述符 fchmod，避免路径被替换成符号链接后修改其他文件权限。
    file.set_permissions(fs::Permissions::from_mode(STAGED_MODE))
        .context("设置基础备份密文权限失败")?;
    file.sync_all().context("同步基础备份密文失败")?;
    let temp = temp_path(directory, backup_id);
    let cipher = base_cipher_path(directory, backup_id);
    let cipher_sha256 = sha256_file(&temp).await?;
    fs::rename(&temp, &cipher)
        .with_context(|| format!("发布基础备份密文失败：{}", cipher.display()))?;
    sync_directory(directory)?;
    let cipher_bytes = fs::symlink_metadata(&cipher)
        .with_context(|| format!("读取基础备份密文失败：{}", cipher.display()))?
        .len();
    let record = BaseManifest {
        format: BASE_MANIFEST_FORMAT.to_owned(),
        backup_id: backup_id.to_owned(),
        started_unix_seconds: started,
        finished_unix_seconds: unix_seconds()?,
        plaintext_bytes: plaintext,
        cipher_bytes,
        cipher_sha256: cipher_sha256.clone(),
    };
    let bytes = serde_json::to_vec_pretty(&record).context("序列化基础备份 manifest 失败")?;
    write_manifest(&base_manifest_path(directory, backup_id), &bytes)?;
    progress(format_args!(
        "wal_base_backup status=created backup_id={backup_id} plaintext={plaintext} cipher={cipher_bytes}"
    ));
    Ok(Outcome {
        backup_id: backup_id.to_owned(),
        status: CreateStatus::Created,
        plaintext_bytes: plaintext,
        cipher_bytes,
        cipher_sha256,
        staging: cipher,
    })
}

/// manifest 是发布证据：create_new、fsync 文件与目录后才返回成功。
fn write_manifest(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .with_context(|| format!("创建基础备份 manifest 失败：{}", path.display()))?;
    file.write_all(bytes).context("写入基础备份 manifest 失败")?;
    file.set_permissions(fs::Permissions::from_mode(STAGED_MODE))
        .context("设置基础备份 manifest 权限失败")?;
    file.sync_all().context("同步基础备份 manifest 失败")?;
    sync_directory(path.parent().context("基础备份 manifest 缺少父目录")?)
}

/// 有界读取 manifest；拒绝符号链接、特殊文件与超出上限的输入。
fn read_manifest(path: &Path) -> anyhow::Result<BaseManifest> {
    let bytes = crate::support::files::read_bounded(path, MAX_MANIFEST_BYTES)?;
    serde_json::from_slice(&bytes).context("解析基础备份 manifest 失败")
}

/// 只接受普通文件；缺失返回 None，符号链接与特殊文件直接失败。
fn regular_or_none(path: &Path) -> anyhow::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            anyhow::bail!("基础备份暂存项必须是普通文件：{}", path.display())
        }
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("读取基础备份暂存项失败：{}", path.display()))
        }
    }
}

/// 固定临时名：同一身份的重试必须先清理上次残留，避免 create_new 永久失败。
fn temp_path(directory: &Path, backup_id: &str) -> PathBuf {
    directory.join(format!("{backup_id}.tar.zst.age.tmp"))
}

fn remove_stale(path: &Path) -> anyhow::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("清理基础备份临时文件失败：{}", path.display()))
        }
    }
}

fn discard(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => progress(format_args!("wal_base_backup 清理临时文件失败：{error}")),
    }
}

fn sync_directory(directory: &Path) -> anyhow::Result<()> {
    File::open(directory)
        .and_then(|handle| handle.sync_all())
        .with_context(|| format!("同步暂存目录失败：{}", directory.display()))
}

/// 转发一段管道并排空对应 stderr；转发一结束就关闭两端，避免上游写满管道后
/// 阻塞在等待 stderr EOF 的 `join!` 上。
async fn forward_and_capture(
    mut reader: impl AsyncRead + Unpin,
    mut writer: impl AsyncWrite + Unpin,
    errors: impl AsyncRead + Unpin + Send + 'static,
) -> (io::Result<u64>, io::Result<(Vec<u8>, bool)>) {
    let capture_task = tokio::spawn(capture(errors, STDERR_LIMIT));
    let copied = tokio::io::copy(&mut reader, &mut writer).await;
    drop(writer);
    drop(reader);
    let captured = match capture_task.await {
        Ok(result) => result,
        Err(error) => Err(io::Error::other(error)),
    };
    (copied, captured)
}

async fn capture(mut stream: impl AsyncRead + Unpin, limit: usize) -> io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let mut overflow = false;
    let mut buffer = [0_u8; 4096];
    loop {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        if bytes.len().saturating_add(count) <= limit {
            bytes.extend_from_slice(&buffer[..count]);
        } else {
            overflow = true;
        }
    }
    Ok((bytes, overflow))
}

fn describe_status(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit {code}"),
        None => "被信号终止".to_owned(),
    }
}

/// 只保留可打印文本并截断；不把无限 stderr 写进日志或回执。
fn describe(captured: io::Result<(Vec<u8>, bool)>) -> String {
    let (bytes, overflow) = match captured {
        Ok(value) => value,
        Err(error) => return format!("读取 stderr 失败：{error}"),
    };
    let text: String = String::from_utf8_lossy(&bytes)
        .chars()
        .filter(|character| !character.is_control())
        .take(512)
        .collect();
    let text = text.trim();
    if text.is_empty() {
        if overflow {
            "stderr 超出上限且无可用文本".to_owned()
        } else {
            "无 stderr 输出".to_owned()
        }
    } else if overflow {
        format!("{text}…")
    } else {
        text.to_owned()
    }
}

fn unix_seconds() -> anyhow::Result<u64> {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .context("系统时间早于 UNIX 纪元")
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsString,
        fs,
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
        path::{Path, PathBuf},
        sync::atomic::{AtomicU32, Ordering},
    };

    use super::{
        CreateStatus, Request, Tool, Tools, base_cipher_path, base_id, base_manifest_path,
        resolve_existing, run, write_cipher,
    };
    use crate::backup::manifest::sha256_file;
    use crate::backup::wal::model::BaseManifest;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "auction-base-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).expect("创建测试根目录");
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                .expect("设置测试根目录权限");
            fs::create_dir(root.join("staging")).expect("创建暂存目录");
            Self { root }
        }

        fn staging(&self) -> PathBuf {
            self.root.join("staging")
        }

        fn source(&self, bytes: &[u8]) -> PathBuf {
            let path = self.root.join("source.tar");
            fs::write(&path, bytes).expect("写入源文件");
            path
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    const RECIPIENT: &str = "age1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq";
    const BACKUP_ID: &str = "base-20261104T031500Z";

    fn tool(program: &str, args: Vec<OsString>) -> Tool {
        Tool { program: OsString::from(program), args, clear_pg_env: false, env: Vec::new() }
    }

    fn tools(source: &Path, source_program: &str) -> Tools {
        Tools {
            source: tool(source_program, vec![source.as_os_str().to_owned()]),
            compress: tool("/bin/cat", Vec::new()),
            encrypt: tool("/bin/cat", Vec::new()),
        }
    }

    fn manifest(path: &Path) -> BaseManifest {
        serde_json::from_slice(&fs::read(path).expect("读取 manifest")).expect("解析 manifest")
    }

    #[tokio::test]
    async fn pipeline_publishes_cipher_and_manifest() {
        let fixture = Fixture::new();
        let source = fixture.source(b"tar-stream-bytes");
        let tools = tools(&source, "/bin/cat");
        let outcome =
            super::publish(&tools, &fixture.staging(), BACKUP_ID, 1_000).await.expect("创建成功");
        assert!(matches!(outcome.status, CreateStatus::Created));
        assert_eq!(outcome.plaintext_bytes, b"tar-stream-bytes".len() as u64);
        let cipher = base_cipher_path(&fixture.staging(), BACKUP_ID);
        assert_eq!(fs::read(&cipher).expect("读取密文"), b"tar-stream-bytes");
        let record = manifest(&base_manifest_path(&fixture.staging(), BACKUP_ID));
        assert!(record.validate(BACKUP_ID).is_ok());
        assert_eq!(record.plaintext_bytes, b"tar-stream-bytes".len() as u64);
        assert!(record.finished_unix_seconds >= record.started_unix_seconds);
        assert_eq!(record.cipher_sha256, sha256_file(&cipher).await.expect("摘要"));
        assert_eq!(record.cipher_bytes, b"tar-stream-bytes".len() as u64);
        assert!(!fixture.staging().join(format!("{BACKUP_ID}.tar.zst.age.tmp")).exists());
        // 同一秒的重跑不重复加密，直接返回既有证据。
        let existing = resolve_existing(&fixture.staging(), BACKUP_ID)
            .expect("判定既有身份")
            .expect("必须命中既有基础备份");
        assert!(matches!(existing.status, CreateStatus::AlreadyCreated));
        assert_eq!(existing.cipher_sha256, record.cipher_sha256);
    }

    #[tokio::test]
    async fn source_failure_leaves_no_published_files() {
        let fixture = Fixture::new();
        let source = fixture.source(b"tar-stream-bytes");
        let tools = tools(&source, "/bin/false");
        let error = super::publish(&tools, &fixture.staging(), BACKUP_ID, 1_000)
            .await
            .expect_err("pg_basebackup 失败必须传播");
        assert!(error.to_string().contains("pg_basebackup"));
        assert!(!base_cipher_path(&fixture.staging(), BACKUP_ID).exists());
        assert!(!base_manifest_path(&fixture.staging(), BACKUP_ID).exists());
        assert!(!fixture.staging().join(format!("{BACKUP_ID}.tar.zst.age.tmp")).exists());
    }

    #[tokio::test]
    async fn compression_failure_leaves_no_published_files() {
        let fixture = Fixture::new();
        let source = fixture.source(b"tar-stream-bytes");
        let mut tools = tools(&source, "/bin/cat");
        tools.compress = tool("/bin/false", Vec::new());
        let error = super::publish(&tools, &fixture.staging(), BACKUP_ID, 1_000)
            .await
            .expect_err("zstd 失败必须传播");
        assert!(error.to_string().contains("zstd"));
        assert!(!base_cipher_path(&fixture.staging(), BACKUP_ID).exists());
        assert!(!base_manifest_path(&fixture.staging(), BACKUP_ID).exists());
    }

    #[tokio::test]
    async fn unconfirmed_cipher_is_discarded_before_creation() {
        let fixture = Fixture::new();
        let cipher = base_cipher_path(&fixture.staging(), BACKUP_ID);
        fs::write(&cipher, b"partial").expect("写入未确认密文");
        assert!(resolve_existing(&fixture.staging(), BACKUP_ID).expect("判定").is_none());
        assert!(!cipher.exists());
    }

    #[tokio::test]
    async fn manifest_without_cipher_fails_closed() {
        let fixture = Fixture::new();
        let manifest_path = base_manifest_path(&fixture.staging(), BACKUP_ID);
        fs::write(&manifest_path, b"{}").expect("写入孤立 manifest");
        let error = resolve_existing(&fixture.staging(), BACKUP_ID)
            .expect_err("孤立 manifest 必须失败关闭");
        assert!(error.to_string().contains("密文缺失"));
    }

    #[tokio::test]
    async fn run_rejects_invalid_recipient_and_relative_directory() {
        let fixture = Fixture::new();
        let invalid_recipient =
            Request { staging_dir: fixture.staging(), recipient: "age1invalid".to_owned() };
        let error = run(&invalid_recipient).await.expect_err("无效收件人必须失败");
        assert!(error.to_string().contains("收件人公钥"));

        let relative = Request {
            staging_dir: PathBuf::from("relative/staging"),
            recipient: RECIPIENT.to_owned(),
        };
        let error = run(&relative).await.expect_err("相对目录必须失败");
        assert!(error.to_string().contains("绝对路径"));
    }

    /// `write_cipher` 需要已打开的临时文件；这里验证同一路径的发布顺序。
    #[tokio::test]
    async fn direct_write_cipher_matches_publish() {
        let fixture = Fixture::new();
        let source = fixture.source(b"bytes");
        let tools = tools(&source, "/bin/cat");
        let temp = fixture.staging().join(format!("{BACKUP_ID}.tar.zst.age.tmp"));
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
            .expect("创建临时文件");
        let outcome =
            write_cipher(&tools, &fixture.staging(), BACKUP_ID, file, 5).await.expect("写入密文");
        assert_eq!(outcome.cipher_bytes, b"bytes".len() as u64);
        assert_eq!(
            fs::read(base_cipher_path(&fixture.staging(), BACKUP_ID)).expect("读取密文"),
            b"bytes"
        );
    }

    #[test]
    fn backup_ids_are_derived_from_utc() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-04T03:15:00Z")
            .expect("解析时刻")
            .with_timezone(&chrono::Utc);
        assert_eq!(base_id(now), BACKUP_ID);
    }
}
