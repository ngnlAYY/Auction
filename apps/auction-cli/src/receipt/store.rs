//! 状态目录下的部署回执存储：每条回执一个 0600 文件，经 [`PrivateDirectory`] 写满临时文件并
//! 同步后以 `RENAME_NOREPLACE` 无覆盖发布；查询只读并拒绝损坏、重复或文件名不符的证据。
//!
//! 读写都相对调用方固定的部署状态目录描述符派生 `receipts/`，不重新解析路径：持锁之后祖先被
//! 改名也不会把回执写进或读自另一棵树。回执目录必须归当前账号且禁止 group/other 访问，每条
//! 回执必须是当前账号所有、单链接、无 group/other 权限的普通文件，否则失败关闭。

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use anyhow::Context;
use uuid::Uuid;

use super::model::{DeploymentReceipt, MAX_RECEIPT_BYTES};
use crate::support::files::PrivateDirectory;

/// 状态目录中存放部署回执的子目录；备份范围把它作为 `deployment_receipts` 收集。
pub(crate) const RECEIPTS_DIRECTORY: &str = "receipts";

/// 回执目录的路径，只用于输出与提示；读写一律经状态目录的描述符。
pub(crate) fn receipts_dir(state_dir: &Path) -> PathBuf {
    state_dir.join(RECEIPTS_DIRECTORY)
}

/// 追加一条回执；同 `receipt_id` 已存在时拒绝，不覆盖任何历史证据。
pub(crate) fn append(
    state: &PrivateDirectory,
    receipt: &DeploymentReceipt,
) -> anyhow::Result<PathBuf> {
    receipt.validate()?;
    let name = file_name(receipt.receipt_id);
    let bytes = serde_json::to_vec_pretty(receipt).context("序列化部署回执失败")?;
    if bytes.len() as u64 > MAX_RECEIPT_BYTES {
        anyhow::bail!("部署回执超过 {MAX_RECEIPT_BYTES} 字节上限");
    }
    let directory = state
        .open_child(RECEIPTS_DIRECTORY, true)
        .context("准备部署回执目录失败")?
        .context("准备部署回执目录失败")?;
    directory.write(&name, &bytes, false).context("发布部署回执失败")?;
    Ok(directory.path.join(name))
}

/// 按完成时间倒序列出回执；目录不存在表示还没有部署回执，不是读取失败。
pub(crate) fn list(
    state: &PrivateDirectory,
    limit: usize,
) -> anyhow::Result<Vec<DeploymentReceipt>> {
    list_matching(state, limit, |_| true)
}

/// 先按 `matches` 过滤全部回执，再按完成时间倒序截取 `limit` 条：过滤发生在限量之前，较新的
/// 其他回执（如大量 `service start/restart`）不会把要找的回执挤出窗口。读取与校验口径同 [`list`]，
/// 任一回执损坏都失败关闭。
pub(crate) fn list_matching(
    state: &PrivateDirectory,
    limit: usize,
    matches: impl Fn(&DeploymentReceipt) -> bool,
) -> anyhow::Result<Vec<DeploymentReceipt>> {
    let Some(directory) =
        state.open_child(RECEIPTS_DIRECTORY, false).context("读取部署回执目录失败")?
    else {
        return Ok(Vec::new());
    };
    // 发布中的临时文件以 `.` 开头；其他扩展名（含无法按 UTF-8 解读的名称）不是回执证据。
    let mut names = directory
        .names()?
        .into_iter()
        .filter(|name| !name.starts_with('.') && name.ends_with(".json"))
        .collect::<Vec<_>>();
    names.sort_unstable();
    let mut receipts = Vec::with_capacity(names.len());
    let mut identifiers = BTreeSet::new();
    for name in names {
        let receipt = read_file(&directory, &name)?;
        if !identifiers.insert(receipt.receipt_id) {
            anyhow::bail!("部署回执目录包含重复的 receipt_id：{name}");
        }
        if matches(&receipt) {
            receipts.push(receipt);
        }
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
pub(crate) fn read(
    state: &PrivateDirectory,
    receipt_id: Uuid,
) -> anyhow::Result<DeploymentReceipt> {
    if receipt_id.is_nil() {
        anyhow::bail!("receipt_id 不能是 nil UUID");
    }
    let directory = state
        .open_child(RECEIPTS_DIRECTORY, false)
        .context("读取部署回执目录失败")?
        .with_context(|| format!("没有部署回执目录：{}", receipts_dir(&state.path).display()))?;
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

/// 相对回执目录的描述符读取；文件必须是当前账号所有、单链接、无 group/other 权限的普通文件。
fn read_file(directory: &PrivateDirectory, name: &str) -> anyhow::Result<DeploymentReceipt> {
    let path = directory.path.join(name);
    let bytes =
        directory.read(name).with_context(|| format!("读取部署回执失败：{}", path.display()))?;
    if bytes.len() as u64 > MAX_RECEIPT_BYTES {
        anyhow::bail!("部署回执超过 {MAX_RECEIPT_BYTES} 字节上限：{}", path.display());
    }
    let receipt: DeploymentReceipt = serde_json::from_slice(&bytes)
        .with_context(|| format!("解析部署回执失败：{}", path.display()))?;
    receipt.validate().with_context(|| format!("部署回执无效：{}", path.display()))?;
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::{MetadataExt, PermissionsExt, symlink},
        path::{Path, PathBuf},
        sync::atomic::{AtomicU32, Ordering},
    };

    use chrono::{Duration, TimeZone};
    use uuid::Uuid;

    use super::{append, list, list_matching, read, receipts_dir};
    use crate::receipt::model::{
        Action, Artifact, DeploymentReceipt, FORMAT, MigrationRecord, Outcome,
    };
    use crate::support::files::PrivateDirectory;

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

        /// 以测试根目录作为部署状态目录，按生产口径核对后固定。
        fn state(&self) -> PrivateDirectory {
            pin(&self.root)
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

    fn pin(path: &Path) -> PrivateDirectory {
        PrivateDirectory::open_trusted(path, true).expect("固定状态目录").expect("状态目录存在")
    }

    /// 显式设定权限，不受测试进程 umask 影响。
    fn make_directory(path: &Path, mode: u32) {
        fs::create_dir_all(path).expect("建立目录");
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("设置权限");
    }

    /// 绕过 `append` 直接写一条回执文件（0600，与正常发布的权限相同）。
    fn write_raw_in(state: &Path, name: &str, bytes: &[u8]) {
        let directory = receipts_dir(state);
        make_directory(&directory, 0o700);
        let path = directory.join(name);
        fs::write(&path, bytes).expect("写入回执");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("设置回执权限");
    }

    fn write_raw(fixture: &Fixture, name: &str, bytes: &[u8]) {
        write_raw_in(&fixture.root, name, bytes);
    }

    #[test]
    fn append_then_read_round_trips_and_is_owner_only() {
        let fixture = Fixture::new();
        let state = fixture.state();
        let receipt = fixture.receipt(0);
        let path = append(&state, &receipt).expect("写入回执");
        let metadata = fs::metadata(&path).expect("读取回执元数据");
        assert_eq!(metadata.mode() & 0o777, 0o600);
        assert_eq!(read(&state, receipt.receipt_id).expect("读取回执"), receipt);
        assert_eq!(list(&state, 10).expect("列出回执"), vec![receipt]);
    }

    #[test]
    fn append_never_overwrites_an_existing_receipt() {
        let fixture = Fixture::new();
        let state = fixture.state();
        let receipt = fixture.receipt(0);
        let _path = append(&state, &receipt).expect("首次写入");
        let error = append(&state, &receipt).expect_err("同 id 必须拒绝覆盖");
        assert!(format!("{error:#}").contains("发布部署回执失败"));
        assert_eq!(read(&state, receipt.receipt_id).expect("原回执仍可读"), receipt);
    }

    #[test]
    fn list_orders_newest_first_and_honours_limit() {
        let fixture = Fixture::new();
        let state = fixture.state();
        let old = fixture.receipt(0);
        let new = fixture.receipt(10);
        append(&state, &old).expect("写入旧回执");
        append(&state, &new).expect("写入新回执");
        let receipts = list(&state, 10).expect("列出回执");
        assert_eq!(receipts, vec![new, old]);
        assert_eq!(list(&state, 1).expect("限制条数").len(), 1);
    }

    /// 过滤先于限量：较新的大量其他回执不会把要找的回执挤出窗口。
    #[test]
    fn list_matching_filters_before_the_limit() {
        let fixture = Fixture::new();
        let state = fixture.state();
        let mut wanted = fixture.receipt(0);
        wanted.action = Action::Start;
        append(&state, &wanted).expect("写入目标回执");
        for minutes in 1..=25 {
            append(&state, &fixture.receipt(minutes)).expect("写入较新的 restart 回执");
        }
        assert!(
            !list(&state, 20).expect("列出回执").contains(&wanted),
            "按完成时间限量时目标回执会被挤出"
        );
        let matched = list_matching(&state, 1, |receipt| receipt.action == Action::Start)
            .expect("过滤后列出");
        assert_eq!(matched, vec![wanted]);
    }

    #[test]
    fn missing_directory_is_an_empty_list_but_read_fails() {
        let fixture = Fixture::new();
        let state = fixture.state();
        assert!(list(&state, 10).expect("没有回执目录").is_empty());
        assert!(read(&state, Uuid::new_v4()).is_err());
    }

    #[test]
    fn corrupt_receipts_fail_closed() {
        let fixture = Fixture::new();
        let state = fixture.state();
        let receipt = fixture.receipt(0);
        write_raw(&fixture, "broken.json", b"not json");
        assert!(list(&state, 10).is_err());

        let mut wrong_format = receipt;
        wrong_format.format = "auction-deployment-receipt.v0".to_owned();
        write_raw(
            &fixture,
            &format!("{}.json", wrong_format.receipt_id),
            &serde_json::to_vec(&wrong_format).unwrap(),
        );
        assert!(list(&state, 10).is_err());
    }

    #[test]
    fn unknown_fields_and_nil_ids_are_rejected() {
        let fixture = Fixture::new();
        let state = fixture.state();
        let receipt = fixture.receipt(0);
        let mut value = serde_json::to_value(&receipt).unwrap();
        value["extra"] = serde_json::json!(true);
        write_raw(
            &fixture,
            &format!("{}.json", receipt.receipt_id),
            &serde_json::to_vec(&value).unwrap(),
        );
        assert!(list(&state, 10).is_err());

        let mut value = serde_json::to_value(fixture.receipt(1)).unwrap();
        value["receipt_id"] = serde_json::json!("00000000-0000-0000-0000-000000000000");
        write_raw(
            &fixture,
            "00000000-0000-0000-0000-000000000000.json",
            &serde_json::to_vec(&value).unwrap(),
        );
        assert!(list(&state, 10).is_err());
    }

    #[test]
    fn filename_and_receipt_id_must_agree() {
        let fixture = Fixture::new();
        let state = fixture.state();
        let receipt = fixture.receipt(0);
        write_raw(
            &fixture,
            &format!("{}.json", Uuid::new_v4()),
            &serde_json::to_vec(&receipt).unwrap(),
        );
        let listed = list(&state, 10).expect("列表按内容标识去重");
        let receipt_id = receipt.receipt_id;
        assert_eq!(listed, vec![receipt]);
        assert!(read(&state, receipt_id).is_err());
    }

    #[test]
    fn duplicate_receipt_ids_and_oversized_files_are_rejected() {
        let fixture = Fixture::new();
        let receipt = fixture.receipt(0);
        let bytes = serde_json::to_vec(&receipt).unwrap();
        write_raw(&fixture, &format!("{}.json", receipt.receipt_id), &bytes);
        write_raw(&fixture, &format!("{}.json", Uuid::new_v4()), &bytes);
        assert!(list(&fixture.state(), 10).is_err());

        let fixture = Fixture::new();
        let oversized = vec![b' '; (crate::receipt::model::MAX_RECEIPT_BYTES as usize) + 1];
        write_raw(&fixture, "oversized.json", &oversized);
        assert!(list(&fixture.state(), 10).is_err());
    }

    #[test]
    fn symlinked_receipts_are_rejected() {
        let fixture = Fixture::new();
        let state = fixture.state();
        let receipt = fixture.receipt(0);
        let target = fixture.root.join("target.json");
        fs::write(&target, serde_json::to_vec(&receipt).unwrap()).expect("写入目标");
        let directory = receipts_dir(&fixture.root);
        make_directory(&directory, 0o700);
        symlink(&target, directory.join(format!("{}.json", receipt.receipt_id)))
            .expect("创建符号链接");
        assert!(list(&state, 10).is_err());
        assert!(read(&state, receipt.receipt_id).is_err());
    }

    /// 状态目录的祖先允许 group 写入时不能固定为状态目录；回执目录或回执文件对 group 开放时
    /// 失败关闭（旧实现只看回执目录本身，祖先可写与回执文件权限都不核对）。
    #[test]
    fn untrusted_ancestors_and_non_private_entries_are_rejected() {
        let fixture = Fixture::new();
        let shared = fixture.root.join("shared-parent");
        make_directory(&shared, 0o770);
        let error = PrivateDirectory::open_trusted(&shared.join("state"), true)
            .err()
            .expect("祖先 group 可写必须拒绝");
        assert!(format!("{error:#}").contains("祖先"), "{error:#}");
        assert!(!shared.join("state").exists(), "拒绝前不创建状态目录");

        let state = fixture.state();
        let receipt = fixture.receipt(0);
        append(&state, &receipt).expect("写入回执");
        let path = receipts_dir(&fixture.root).join(format!("{}.json", receipt.receipt_id));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).expect("放宽回执文件");
        assert!(list(&state, 10).is_err(), "回执文件必须 owner-only");
        assert!(read(&state, receipt.receipt_id).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("恢复回执文件");

        fs::set_permissions(receipts_dir(&fixture.root), fs::Permissions::from_mode(0o750))
            .expect("放宽回执目录");
        let error = append(&state, &fixture.receipt(1)).expect_err("回执目录必须 owner-only");
        assert!(format!("{error:#}").contains("准备部署回执目录失败"), "{error:#}");
    }

    /// 持锁期间祖先被改名、原路径换上另一棵带伪造回执的树：读写仍落在固定的状态目录，既不读到
    /// 新树里的回执，也不把新回执写进新树。
    #[test]
    fn receipts_stay_with_the_pinned_state_directory() {
        let fixture = Fixture::new();
        let parent = fixture.root.join("lib");
        make_directory(&parent, 0o755);
        let state_path = parent.join("state");
        let state = pin(&state_path);
        let kept = fixture.receipt(0);
        append(&state, &kept).expect("写入回执");

        let moved = fixture.root.join("lib.moved");
        fs::rename(&parent, &moved).expect("改名祖先");
        make_directory(&state_path, 0o700);
        let planted = fixture.receipt(5);
        write_raw_in(
            &state_path,
            &format!("{}.json", planted.receipt_id),
            &serde_json::to_vec(&planted).unwrap(),
        );

        assert_eq!(list(&state, 10).expect("列出回执"), vec![kept]);
        assert!(read(&state, planted.receipt_id).is_err(), "不读另一棵树里的回执");
        let later = fixture.receipt(10);
        append(&state, &later).expect("写入回执");
        let name = format!("{}.json", later.receipt_id);
        assert!(receipts_dir(&moved.join("state")).join(&name).exists());
        assert!(!receipts_dir(&state_path).join(&name).exists(), "不写进另一棵树");
        assert!(state.verify_path().is_err(), "路径已不指向固定的状态目录");
    }

    #[test]
    fn append_rejects_invalid_receipts_before_writing() {
        let fixture = Fixture::new();
        let mut receipt = fixture.receipt(0);
        receipt.actor = "bad\nactor".to_owned();
        assert!(append(&fixture.state(), &receipt).is_err());
        assert!(!receipts_dir(&fixture.root).exists());
    }
}
