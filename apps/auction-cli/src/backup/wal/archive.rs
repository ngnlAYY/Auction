//! PostgreSQL `archive_command` 的本机实现：zstd 压缩、age 公钥加密，fsync 后原子发布到暂存目录。
//!
//! 任一步失败都以非零退出，由 PostgreSQL 重试同一段；本命令不访问数据库、不读敏感 TOML，
//! 因此可以按 PostgreSQL 服务账号执行。私钥离线保存，服务器只有收件人公钥。

use std::{
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    time::SystemTime,
};

use anyhow::Context;
use serde::Serialize;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};

use super::model::{
    ArchiveManifest, CIPHER_SUFFIX, Duplicate, MANIFEST_FORMAT, SourceIdentity, cipher_path,
    classify, manifest_path, require_staging_dir, source_identity, valid_recipient,
    valid_segment_name,
};
use crate::app::progress;

/// 子进程 stderr 的有界捕获上限；失败原因进入 PostgreSQL 日志，不进入任何回执文件。
const STDERR_LIMIT: usize = 4096;
/// 暂存文件允许同组读取：归档账号（postgres）与上传账号可能不同。
const STAGED_MODE: u32 = 0o640;

/// 与 `archive_command` 的参数一一对应；`source` 是 `%p`，`name` 是 `%f`。
pub(super) struct Request {
    pub(super) staging_dir: PathBuf,
    pub(super) recipient: String,
    pub(super) source: PathBuf,
    pub(super) name: String,
}

/// 归档命令的机器可读结果；重复归档与首次归档都退出 0。
#[derive(Debug, Serialize)]
pub(super) struct Outcome {
    pub(super) segment_name: String,
    pub(super) status: ArchiveStatus,
    pub(super) source_bytes: u64,
    pub(super) cipher_bytes: u64,
    pub(super) staging: PathBuf,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ArchiveStatus {
    Archived,
    AlreadyArchived,
}

/// 校验输入后按既有 manifest 判定重复，再执行压缩与加密。
pub(super) async fn run(request: &Request) -> anyhow::Result<Outcome> {
    require_staging_dir(&request.staging_dir)?;
    if !valid_recipient(&request.recipient) {
        anyhow::bail!("age 收件人公钥无效；须与 [backup.offsite] age_recipient 相同");
    }
    if !valid_segment_name(&request.name) {
        anyhow::bail!("WAL 段名无效：{}", request.name);
    }
    let source_name =
        request.source.file_name().and_then(OsStr::to_str).context("WAL 源路径缺少文件名")?;
    if source_name != request.name {
        anyhow::bail!("WAL 段名必须等于源文件名：{} != {}", request.name, source_name);
    }
    let source = source_identity(&request.source)?;
    if let Some(outcome) = resolve_duplicate(&request.staging_dir, &request.name, &source)? {
        return Ok(outcome);
    }
    let compress = Tool {
        program: OsString::from("zstd"),
        args: vec![
            OsString::from("-q"),
            OsString::from("-T1"),
            OsString::from("-c"),
            OsString::from("--"),
            request.source.clone().into_os_string(),
        ],
    };
    let encrypt = Tool {
        program: OsString::from("age"),
        args: vec![
            OsString::from("--encrypt"),
            OsString::from("--recipient"),
            OsString::from(request.recipient.as_str()),
        ],
    };
    archive_with(&compress, &encrypt, &request.staging_dir, &request.name, &source).await
}

/// 按既有证据决定是否直接成功；未确认的密文在这里清理，返回 `None` 表示需要归档。
fn resolve_duplicate(
    directory: &Path,
    segment: &str,
    source: &SourceIdentity,
) -> anyhow::Result<Option<Outcome>> {
    match classify(directory, segment, source)? {
        Duplicate::Confirmed(record) => {
            progress(format_args!("wal_archive status=already_archived segment={segment}"));
            Ok(Some(Outcome {
                segment_name: segment.to_owned(),
                status: ArchiveStatus::AlreadyArchived,
                source_bytes: source.bytes,
                cipher_bytes: record.cipher_bytes,
                staging: cipher_path(directory, segment),
            }))
        }
        Duplicate::UnconfirmedCipher(cipher) => {
            // 密文从未随 manifest 发布，PostgreSQL 也未收到成功退出；删除后重新归档。
            fs::remove_file(&cipher)
                .with_context(|| format!("清理未确认的 WAL 密文失败：{}", cipher.display()))?;
            sync_directory(directory)?;
            progress(format_args!("wal_archive status=discarded_unconfirmed segment={segment}"));
            Ok(None)
        }
        Duplicate::Absent => Ok(None),
    }
}

/// 一个外部过滤程序；测试注入 `/bin/cat` 或 `/bin/false` 验证发布与失败传播。
struct Tool {
    program: OsString,
    args: Vec<OsString>,
}

/// 写入临时文件，两个过滤程序都成功后 fsync、改名并发布 manifest。
async fn archive_with(
    compress: &Tool,
    encrypt: &Tool,
    directory: &Path,
    segment: &str,
    source: &SourceIdentity,
) -> anyhow::Result<Outcome> {
    let temp = temp_path(directory, segment);
    remove_stale(&temp)?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&temp)
        .with_context(|| format!("创建 WAL 密文临时文件失败：{}", temp.display()))?;
    let result = publish(compress, encrypt, directory, segment, file, source).await;
    if result.is_err() {
        // 只回收本次未发布的临时文件；已发布的密文由下次运行按 manifest 判定。
        discard(&temp);
    }
    result
}

async fn publish(
    compress: &Tool,
    encrypt: &Tool,
    directory: &Path,
    segment: &str,
    file: File,
    source: &SourceIdentity,
) -> anyhow::Result<Outcome> {
    let temp = temp_path(directory, segment);
    let cipher = cipher_path(directory, segment);
    let mut compress_child = Command::new(&compress.program)
        .args(&compress.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| {
            format!("无法启动 {}；请确认归档账号 PATH", compress.program.to_string_lossy())
        })?;
    let mut compressed = compress_child.stdout.take().context("压缩输出管道不可用")?;
    let compress_errors = compress_child.stderr.take().context("压缩错误管道不可用")?;
    let mut encrypt_child = Command::new(&encrypt.program)
        .args(&encrypt.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(file.try_clone().context("复制 WAL 密文句柄失败")?))
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| {
            format!("无法启动 {}；请确认归档账号 PATH", encrypt.program.to_string_lossy())
        })?;
    let mut encrypt_input = encrypt_child.stdin.take().context("加密输入管道不可用")?;
    let encrypt_errors = encrypt_child.stderr.take().context("加密错误管道不可用")?;
    // 转发压缩流并同时排空压缩 stderr；加密侧 stderr 只在 stdin 关闭后才读到 EOF。
    let (forwarded, compress_text) = tokio::join!(
        tokio::io::copy(&mut compressed, &mut encrypt_input),
        capture(compress_errors, STDERR_LIMIT),
    );
    drop(encrypt_input);
    // 关闭压缩读端：加密提前退出时压缩进程会收到 EPIPE，而不是阻塞在写满的管道上。
    drop(compressed);
    let (encrypt_status, encrypt_text) =
        tokio::join!(encrypt_child.wait(), capture(encrypt_errors, STDERR_LIMIT));
    let encrypt_status = encrypt_status.context("等待加密进程失败")?;
    let compress_status = compress_child.wait().await.context("等待压缩进程失败")?;
    // 加密先判定：加密提前退出导致的 SIGPIPE 不能误报成压缩失败；但压缩确实失败时
    // age 可能已对截断的输入产出合法密文，因此两者都要检查。
    if !encrypt_status.success() {
        anyhow::bail!(
            "age 加密失败（{}）：{}",
            describe_status(encrypt_status),
            describe(encrypt_text)
        );
    }
    if !compress_status.success() {
        anyhow::bail!(
            "zstd 压缩失败（{}）：{}",
            describe_status(compress_status),
            describe(compress_text)
        );
    }
    forwarded.context("转发压缩输出到 age 失败")?;
    // 用文件描述符 fchmod，避免路径被替换成符号链接后修改其他文件权限。
    file.set_permissions(fs::Permissions::from_mode(STAGED_MODE))
        .context("设置 WAL 密文权限失败")?;
    file.sync_all().context("同步 WAL 密文失败")?;
    fs::rename(temp, &cipher)
        .with_context(|| format!("发布 WAL 密文失败：{}", cipher.display()))?;
    sync_directory(directory)?;
    let cipher_bytes = fs::symlink_metadata(&cipher)
        .with_context(|| format!("读取 WAL 密文失败：{}", cipher.display()))?
        .len();
    let record = ArchiveManifest {
        format: MANIFEST_FORMAT.to_owned(),
        segment_name: segment.to_owned(),
        source_bytes: source.bytes,
        source_mtime_ns: source.mtime_ns,
        cipher_bytes,
        archived_unix_seconds: unix_seconds()?,
    };
    let bytes = serde_json::to_vec_pretty(&record).context("序列化 WAL 归档 manifest 失败")?;
    write_manifest(&manifest_path(directory, segment), &bytes)?;
    progress(format_args!("wal_archive status=archived segment={segment} bytes={cipher_bytes}"));
    Ok(Outcome {
        segment_name: segment.to_owned(),
        status: ArchiveStatus::Archived,
        source_bytes: source.bytes,
        cipher_bytes,
        staging: cipher.to_path_buf(),
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
        .with_context(|| format!("创建 WAL 归档 manifest 失败：{}", path.display()))?;
    file.write_all(bytes).context("写入 WAL 归档 manifest 失败")?;
    file.set_permissions(fs::Permissions::from_mode(STAGED_MODE))
        .context("设置 WAL 归档 manifest 权限失败")?;
    file.sync_all().context("同步 WAL 归档 manifest 失败")?;
    sync_directory(path.parent().context("WAL 归档 manifest 缺少父目录")?)
}

/// 固定临时名：同一段的重试必须先清理上次残留，避免 create_new 永久失败。
fn temp_path(directory: &Path, segment: &str) -> PathBuf {
    directory.join(format!("{segment}{CIPHER_SUFFIX}.tmp"))
}

fn remove_stale(path: &Path) -> anyhow::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("清理 WAL 临时文件失败：{}", path.display()))
        }
    }
}

fn discard(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => progress(format_args!("wal_archive 清理临时文件失败：{error}")),
    }
}

fn sync_directory(directory: &Path) -> anyhow::Result<()> {
    File::open(directory)
        .and_then(|handle| handle.sync_all())
        .with_context(|| format!("同步 WAL 暂存目录失败：{}", directory.display()))
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
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU32, Ordering},
    };

    use super::{
        ArchiveStatus, Duplicate, Request, Tool, archive_with, cipher_path, classify,
        manifest_path, resolve_duplicate, run, source_identity, temp_path,
    };

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "auction-wal-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).expect("创建测试根目录");
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                .expect("设置测试根目录权限");
            fs::create_dir(root.join("staging")).expect("创建暂存目录");
            fs::create_dir(root.join("pg_wal")).expect("创建源目录");
            Self { root }
        }

        fn staging(&self) -> PathBuf {
            self.root.join("staging")
        }

        fn source(&self, segment: &str, bytes: &[u8]) -> PathBuf {
            let path = self.root.join("pg_wal").join(segment);
            fs::write(&path, bytes).expect("写入源文件");
            path
        }

        fn request(&self, source: &Path, segment: &str) -> Request {
            Request {
                staging_dir: self.staging(),
                recipient: RECIPIENT.to_owned(),
                source: source.to_path_buf(),
                name: segment.to_owned(),
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    const RECIPIENT: &str = "age1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq";
    const SEGMENT: &str = "000000010000000000000001";

    fn copy_tool() -> Tool {
        Tool { program: OsString::from("/bin/cat"), args: Vec::new() }
    }

    fn source_copy_tool(source: &Path) -> Tool {
        Tool { program: OsString::from("/bin/cat"), args: vec![source.as_os_str().to_owned()] }
    }

    fn failing_tool() -> Tool {
        Tool { program: OsString::from("/bin/false"), args: Vec::new() }
    }

    #[tokio::test]
    async fn archive_publishes_manifest_and_reports_duplicate() {
        let fixture = Fixture::new();
        let source = fixture.source(SEGMENT, b"wal-segment-bytes");
        let identity = source_identity(&source).expect("读取源身份");

        let first = archive_with(
            &source_copy_tool(&source),
            &copy_tool(),
            &fixture.staging(),
            SEGMENT,
            &identity,
        )
        .await
        .expect("首次归档成功");
        assert!(matches!(first.status, ArchiveStatus::Archived));
        assert_eq!(first.cipher_bytes, b"wal-segment-bytes".len() as u64);
        assert_eq!(
            fs::read(cipher_path(&fixture.staging(), SEGMENT)).expect("读取密文"),
            b"wal-segment-bytes"
        );

        let record = match classify(&fixture.staging(), SEGMENT, &identity).expect("判定重复") {
            Duplicate::Confirmed(record) => record,
            other => panic!("重复归档必须命中 manifest：{other:?}"),
        };
        assert_eq!(record.source_bytes, b"wal-segment-bytes".len() as u64);
        assert_eq!(record.cipher_bytes, b"wal-segment-bytes".len() as u64);

        let again = run(&fixture.request(&source, SEGMENT)).await.expect("重复归档成功退出");
        assert!(matches!(again.status, ArchiveStatus::AlreadyArchived));
        assert_eq!(again.cipher_bytes, first.cipher_bytes);
    }

    #[tokio::test]
    async fn compress_failure_propagates_and_leaves_no_published_files() {
        let fixture = Fixture::new();
        let source = fixture.source(SEGMENT, b"wal-segment-bytes");
        let identity = source_identity(&source).expect("读取源身份");

        let error =
            archive_with(&failing_tool(), &copy_tool(), &fixture.staging(), SEGMENT, &identity)
                .await
                .expect_err("压缩失败必须传播");
        assert!(error.to_string().contains("zstd"));
        assert!(!cipher_path(&fixture.staging(), SEGMENT).exists());
        assert!(!manifest_path(&fixture.staging(), SEGMENT).exists());
        assert!(!temp_path(&fixture.staging(), SEGMENT).exists());
    }

    #[tokio::test]
    async fn unconfirmed_cipher_is_discarded_and_rewritten() {
        let fixture = Fixture::new();
        let source = fixture.source(SEGMENT, b"new-bytes");
        let identity = source_identity(&source).expect("读取源身份");
        let cipher = cipher_path(&fixture.staging(), SEGMENT);
        fs::write(&cipher, b"stale").expect("写入未确认密文");

        // 只有密文而没有 manifest：按上次中断处理，删除后重新归档。
        assert!(
            resolve_duplicate(&fixture.staging(), SEGMENT, &identity)
                .expect("判定暂存状态")
                .is_none()
        );
        assert!(!cipher.exists());

        let outcome = archive_with(
            &source_copy_tool(&source),
            &copy_tool(),
            &fixture.staging(),
            SEGMENT,
            &identity,
        )
        .await
        .expect("重新归档成功");
        assert!(matches!(outcome.status, ArchiveStatus::Archived));
        assert_eq!(fs::read(&cipher).expect("读取密文"), b"new-bytes");
        assert!(manifest_path(&fixture.staging(), SEGMENT).exists());
    }

    #[tokio::test]
    async fn changed_source_content_fails_closed() {
        let fixture = Fixture::new();
        let source = fixture.source(SEGMENT, b"first-bytes");
        let identity = source_identity(&source).expect("读取源身份");
        archive_with(
            &source_copy_tool(&source),
            &copy_tool(),
            &fixture.staging(),
            SEGMENT,
            &identity,
        )
        .await
        .expect("首次归档成功");

        fixture.source(SEGMENT, b"different-length-bytes");
        let error =
            run(&fixture.request(&source, SEGMENT)).await.expect_err("内容变化必须失败关闭");
        assert!(error.to_string().contains("不同源内容"));
        assert_eq!(
            fs::read(cipher_path(&fixture.staging(), SEGMENT)).expect("读取原密文"),
            b"first-bytes"
        );
    }

    #[tokio::test]
    async fn segment_name_must_match_source_file_name() {
        let fixture = Fixture::new();
        let source = fixture.source(SEGMENT, b"bytes");
        let mut request = fixture.request(&source, SEGMENT);
        request.name = "000000010000000000000002".to_owned();
        let error = run(&request).await.expect_err("段名不一致必须失败");
        assert!(error.to_string().contains("段名必须等于源文件名"));
    }
}
