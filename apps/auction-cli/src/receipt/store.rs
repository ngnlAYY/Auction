//! 状态目录下的部署回执存储：每条回执一个 0600 文件，临时文件同步后以
//! `RENAME_NOREPLACE` 无覆盖发布；查询只读并拒绝损坏、重复或文件名不符的证据。

use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::Write,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

use anyhow::Context;
use uuid::Uuid;

use super::model::{DeploymentReceipt, MAX_RECEIPT_BYTES};
use crate::support::files;

/// 状态目录中存放部署回执的子目录；备份范围把它作为 `deployment_receipts` 收集。
pub(crate) const RECEIPTS_DIRECTORY: &str = "receipts";

pub(crate) fn receipts_dir(state_dir: &Path) -> PathBuf {
    state_dir.join(RECEIPTS_DIRECTORY)
}

/// 追加一条回执；同 `receipt_id` 已存在时拒绝，不覆盖任何历史证据。
pub(crate) fn append(state_dir: &Path, receipt: &DeploymentReceipt) -> anyhow::Result<PathBuf> {
    receipt.validate()?;
    let directory = receipts_dir(state_dir);
    files::ensure_private_dir(&directory).context("准备部署回执目录失败")?;
    let name = file_name(receipt.receipt_id);
    let bytes = serde_json::to_vec_pretty(receipt).context("序列化部署回执失败")?;
    if bytes.len() as u64 > MAX_RECEIPT_BYTES {
        anyhow::bail!("部署回执超过 {MAX_RECEIPT_BYTES} 字节上限");
    }
    publish_noreplace(&directory, &name, &bytes)?;
    Ok(directory.join(name))
}

/// 按完成时间倒序列出回执；目录不存在表示还没有部署回执，不是读取失败。
pub(crate) fn list(state_dir: &Path, limit: usize) -> anyhow::Result<Vec<DeploymentReceipt>> {
    let directory = receipts_dir(state_dir);
    if !require_directory(&directory)? {
        return Ok(Vec::new());
    }
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("读取部署回执目录失败：{}", directory.display()));
        }
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.context("读取部署回执目录项失败")?;
        let name = entry.file_name().into_string().map_err(|_: std::ffi::OsString| {
            anyhow::anyhow!("部署回执目录包含非 UTF-8 文件名：{}", directory.display())
        })?;
        // 发布中的临时文件以 `.` 开头；其他扩展名不是回执证据。
        if name.starts_with('.') || !name.ends_with(".json") {
            continue;
        }
        names.push(name);
    }
    names.sort_unstable();
    let mut receipts = Vec::with_capacity(names.len());
    let mut identifiers = BTreeSet::new();
    for name in names {
        let receipt = read_file(&directory, &name)?;
        if !identifiers.insert(receipt.receipt_id) {
            anyhow::bail!("部署回执目录包含重复的 receipt_id：{name}");
        }
        receipts.push(receipt);
    }
    receipts.sort_by(|left, right| {
        right
            .finished_at
            .cmp(&left.finished_at)
            .then_with(|| right.receipt_id.cmp(&left.receipt_id))
    });
    receipts.truncate(limit);
    Ok(receipts)
}

/// 按回执标识读取单条证据；缺失或损坏都失败，不返回部分记录。
pub(crate) fn read(state_dir: &Path, receipt_id: Uuid) -> anyhow::Result<DeploymentReceipt> {
    if receipt_id.is_nil() {
        anyhow::bail!("receipt_id 不能是 nil UUID");
    }
    let directory = receipts_dir(state_dir);
    if !require_directory(&directory)? {
        anyhow::bail!("没有部署回执目录：{}", directory.display());
    }
    let name = file_name(receipt_id);
    let receipt = read_file(&directory, &name)?;
    if receipt.receipt_id != receipt_id {
        anyhow::bail!("部署回执内容的 receipt_id 与文件名不一致：{name}");
    }
    Ok(receipt)
}

fn file_name(receipt_id: Uuid) -> String {
    format!("{receipt_id}.json")
}

/// 返回 false 表示目录尚不存在；符号链接或非目录一律失败。
fn require_directory(directory: &Path) -> anyhow::Result<bool> {
    let metadata = match fs::symlink_metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("读取部署回执目录失败：{}", directory.display()));
        }
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        anyhow::bail!("部署回执路径必须是非符号链接目录：{}", directory.display());
    }
    Ok(true)
}

fn read_file(directory: &Path, name: &str) -> anyhow::Result<DeploymentReceipt> {
    let path = directory.join(name);
    let bytes = files::read_bounded(&path, MAX_RECEIPT_BYTES)
        .with_context(|| format!("读取部署回执失败：{}", path.display()))?;
    let receipt: DeploymentReceipt = serde_json::from_slice(&bytes)
        .with_context(|| format!("解析部署回执失败：{}", path.display()))?;
    receipt.validate().with_context(|| format!("部署回执无效：{}", path.display()))?;
    Ok(receipt)
}

/// 目录内无覆盖发布：0600 临时文件写满并同步后原子改名，再同步目录。
fn publish_noreplace(directory: &Path, name: &str, bytes: &[u8]) -> anyhow::Result<()> {
    name_component(name)?;
    let directory_file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(directory)
        .with_context(|| format!("打开部署回执目录失败：{}", directory.display()))?;
    require_private_owner(&directory_file, directory)?;
    let temporary = format!(".{}.partial", Uuid::new_v4());
    name_component(&temporary)?;
    let mut temporary_created = false;
    let result = (|| {
        let mut file =
            open_at(&directory_file, &temporary, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)?;
        temporary_created = true;
        file.write_all(bytes).context("写入部署回执失败")?;
        file.sync_all().context("同步部署回执失败")?;
        let name = std::ffi::CString::new(name).context("部署回执文件名含 NUL")?;
        let temporary = std::ffi::CString::new(temporary.as_str()).context("临时文件名含 NUL")?;
        // SAFETY：目录描述符在本次调用期间有效，两个名称都是无 NUL 的单路径分量；
        // 禁止覆盖已有回执，历史证据只能追加。
        if unsafe {
            libc::renameat2(
                directory_file.as_raw_fd(),
                temporary.as_ptr(),
                directory_file.as_raw_fd(),
                name.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error()).context("发布部署回执失败");
        }
        Ok(())
    })();
    if temporary_created && result.is_err() {
        let _ = unlink_at(&directory_file, &temporary);
    }
    result?;
    directory_file.sync_all().context("同步部署回执目录失败")
}

fn open_at(directory: &File, name: &str, flags: i32) -> anyhow::Result<File> {
    let name = name_component(name)?;
    // SAFETY：目录描述符有效，名称已限制为单分量；成功返回的 fd 只交给 File 一次。
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error()).context("创建部署回执临时文件失败");
    }
    // SAFETY：openat 成功返回新的独占描述符。
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

fn unlink_at(directory: &File, name: &str) -> std::io::Result<()> {
    let name = std::ffi::CString::new(name).map_err(|_: std::ffi::NulError| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "临时文件名含 NUL")
    })?;
    // SAFETY：目录描述符有效，名称为本次创建的单分量文件。
    if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn require_private_owner(directory: &File, path: &Path) -> anyhow::Result<()> {
    let metadata = directory.metadata().context("读取部署回执目录元数据失败")?;
    if metadata.uid() != files::effective_uid() || metadata.mode() & 0o077 != 0 {
        anyhow::bail!("部署回执目录必须归当前账号所有且禁止 group/other 访问：{}", path.display());
    }
    Ok(())
}

fn name_component(name: &str) -> anyhow::Result<std::ffi::CString> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.chars().any(char::is_control)
    {
        anyhow::bail!("部署回执文件名必须是受控单路径分量");
    }
    std::ffi::CString::new(name).context("部署回执文件名含 NUL")
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::{MetadataExt, PermissionsExt, symlink},
        path::PathBuf,
        sync::atomic::{AtomicU32, Ordering},
    };

    use chrono::{Duration, TimeZone};
    use uuid::Uuid;

    use super::{append, list, read, receipts_dir};
    use crate::receipt::model::{
        Action, Artifact, DeploymentReceipt, FORMAT, MigrationRecord, Outcome,
    };

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "auction-deployment-receipts-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).expect("创建测试根目录");
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                .expect("设置测试根目录权限");
            Self { root }
        }

        fn receipt(&self, minutes: i64) -> DeploymentReceipt {
            let failed = minutes < 0;
            let started = chrono::Utc.with_ymd_and_hms(2026, 11, 5, 3, 0, 0).unwrap()
                + Duration::minutes(minutes);
            DeploymentReceipt {
                format: FORMAT.to_owned(),
                receipt_id: Uuid::new_v4(),
                action: Action::Restart,
                outcome: if failed { Outcome::Failed } else { Outcome::Succeeded },
                started_at: started,
                finished_at: started + Duration::minutes(1),
                actor: "root".to_owned(),
                actor_uid: 0,
                backup_id: Some("backup-20261105".to_owned()),
                migration: Some(MigrationRecord {
                    before_version: 95,
                    after_version: 95,
                    expected_version: 95,
                    applied: 0,
                }),
                artifacts: vec![Artifact {
                    role: "worker".to_owned(),
                    path: Some("/opt/auction/bin/auction-worker".to_owned()),
                    sha256: Some("a".repeat(64)),
                    bytes: Some(4096),
                }],
                failure: failed.then(|| "systemctl start 失败".to_owned()),
                deploy: None,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn write_raw(fixture: &Fixture, name: &str, bytes: &[u8]) {
        let directory = receipts_dir(&fixture.root);
        fs::create_dir_all(&directory).expect("创建回执目录");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .expect("设置回执目录权限");
        fs::write(directory.join(name), bytes).expect("写入回执");
    }

    #[test]
    fn append_then_read_round_trips_and_is_owner_only() {
        let fixture = Fixture::new();
        let receipt = fixture.receipt(0);
        let path = append(&fixture.root, &receipt).expect("写入回执");
        let metadata = fs::metadata(&path).expect("读取回执元数据");
        assert_eq!(metadata.mode() & 0o777, 0o600);
        assert_eq!(read(&fixture.root, receipt.receipt_id).expect("读取回执"), receipt);
        assert_eq!(list(&fixture.root, 10).expect("列出回执"), vec![receipt]);
    }

    #[test]
    fn append_never_overwrites_an_existing_receipt() {
        let fixture = Fixture::new();
        let receipt = fixture.receipt(0);
        let _path = append(&fixture.root, &receipt).expect("首次写入");
        let error = append(&fixture.root, &receipt).expect_err("同 id 必须拒绝覆盖");
        assert!(format!("{error:#}").contains("发布部署回执失败"));
        assert_eq!(read(&fixture.root, receipt.receipt_id).expect("原回执仍可读"), receipt);
    }

    #[test]
    fn list_orders_newest_first_and_honours_limit() {
        let fixture = Fixture::new();
        let old = fixture.receipt(0);
        let new = fixture.receipt(10);
        append(&fixture.root, &old).expect("写入旧回执");
        append(&fixture.root, &new).expect("写入新回执");
        let receipts = list(&fixture.root, 10).expect("列出回执");
        assert_eq!(receipts, vec![new, old]);
        assert_eq!(list(&fixture.root, 1).expect("限制条数").len(), 1);
    }

    #[test]
    fn missing_directory_is_an_empty_list_but_read_fails() {
        let fixture = Fixture::new();
        assert!(list(&fixture.root, 10).expect("没有回执目录").is_empty());
        assert!(read(&fixture.root, Uuid::new_v4()).is_err());
    }

    #[test]
    fn corrupt_receipts_fail_closed() {
        let fixture = Fixture::new();
        let receipt = fixture.receipt(0);
        write_raw(&fixture, "broken.json", b"not json");
        assert!(list(&fixture.root, 10).is_err());

        let mut wrong_format = receipt.clone();
        wrong_format.format = "auction-deployment-receipt.v0".to_owned();
        write_raw(
            &fixture,
            &format!("{}.json", wrong_format.receipt_id),
            &serde_json::to_vec(&wrong_format).unwrap(),
        );
        assert!(list(&fixture.root, 10).is_err());
    }

    #[test]
    fn unknown_fields_and_nil_ids_are_rejected() {
        let fixture = Fixture::new();
        let receipt = fixture.receipt(0);
        let mut value = serde_json::to_value(&receipt).unwrap();
        value["extra"] = serde_json::json!(true);
        write_raw(
            &fixture,
            &format!("{}.json", receipt.receipt_id),
            &serde_json::to_vec(&value).unwrap(),
        );
        assert!(list(&fixture.root, 10).is_err());

        let mut value = serde_json::to_value(fixture.receipt(1)).unwrap();
        value["receipt_id"] = serde_json::json!("00000000-0000-0000-0000-000000000000");
        write_raw(
            &fixture,
            "00000000-0000-0000-0000-000000000000.json",
            &serde_json::to_vec(&value).unwrap(),
        );
        assert!(list(&fixture.root, 10).is_err());
    }

    #[test]
    fn filename_and_receipt_id_must_agree() {
        let fixture = Fixture::new();
        let receipt = fixture.receipt(0);
        write_raw(
            &fixture,
            &format!("{}.json", Uuid::new_v4()),
            &serde_json::to_vec(&receipt).unwrap(),
        );
        let listed = list(&fixture.root, 10).expect("列表按内容标识去重");
        let receipt_id = receipt.receipt_id;
        assert_eq!(listed, vec![receipt]);
        assert!(read(&fixture.root, receipt_id).is_err());
    }

    #[test]
    fn duplicate_receipt_ids_and_oversized_files_are_rejected() {
        let fixture = Fixture::new();
        let receipt = fixture.receipt(0);
        let bytes = serde_json::to_vec(&receipt).unwrap();
        write_raw(&fixture, &format!("{}.json", receipt.receipt_id), &bytes);
        write_raw(&fixture, &format!("{}.json", Uuid::new_v4()), &bytes);
        assert!(list(&fixture.root, 10).is_err());

        let fixture = Fixture::new();
        let oversized = vec![b' '; (crate::receipt::model::MAX_RECEIPT_BYTES as usize) + 1];
        write_raw(&fixture, "oversized.json", &oversized);
        assert!(list(&fixture.root, 10).is_err());
    }

    #[test]
    fn symlinked_receipts_are_rejected() {
        let fixture = Fixture::new();
        let receipt = fixture.receipt(0);
        let target = fixture.root.join("target.json");
        fs::write(&target, serde_json::to_vec(&receipt).unwrap()).expect("写入目标");
        let directory = receipts_dir(&fixture.root);
        fs::create_dir_all(&directory).expect("创建回执目录");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).expect("设置目录权限");
        symlink(&target, directory.join(format!("{}.json", receipt.receipt_id)))
            .expect("创建符号链接");
        assert!(list(&fixture.root, 10).is_err());
        assert!(read(&fixture.root, receipt.receipt_id).is_err());
    }

    #[test]
    fn append_rejects_invalid_receipts_before_writing() {
        let fixture = Fixture::new();
        let mut receipt = fixture.receipt(0);
        receipt.actor = "bad\nactor".to_owned();
        assert!(append(&fixture.root, &receipt).is_err());
        assert!(!receipts_dir(&fixture.root).exists());
    }
}
