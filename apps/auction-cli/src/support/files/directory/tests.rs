//! 私有目录的路径、权限与发布不变量测试。

use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

use uuid::Uuid;

use super::PrivateDirectory;

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("auction-private-dir-{}", Uuid::new_v4()));
        fs::create_dir(&root).expect("建立临时目录");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("设置权限");
        Self(root)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// 无覆盖发布不改写已有证据；可替换发布用于运行中状态。
#[test]
fn write_never_overwrites_without_replace() {
    let root = TempRoot::new();
    let directory = PrivateDirectory::prepare(&root.0.join("receipts")).expect("准备私有目录");
    directory.write("a.json", b"first", false).expect("首次发布");
    assert!(directory.write("a.json", b"second", false).is_err());
    assert_eq!(directory.read("a.json").expect("读取"), b"first");
    directory.write("a.json", b"third", true).expect("可替换发布");
    assert_eq!(directory.read("a.json").expect("读取"), b"third");
    let names = directory.names().expect("枚举");
    assert_eq!(names, vec!["a.json".to_owned()], "失败与成功路径都不残留临时名");
}

/// 祖先目录允许 group 写入时拒绝准备私有目录：他人可以替换其中的目录项。
#[test]
fn group_writable_ancestor_is_rejected() {
    let root = TempRoot::new();
    let shared = root.0.join("shared");
    fs::create_dir(&shared).expect("建立祖先目录");
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o770)).expect("设置权限");
    let error =
        PrivateDirectory::prepare(&shared.join("receipts")).err().expect("group 可写祖先必须拒绝");
    assert!(format!("{error:#}").contains("祖先"), "{error:#}");
    assert!(!shared.join("receipts").exists(), "拒绝前不创建目录");
}

/// `open_existing` 只核对目录本身：祖先对 group 可写不影响打开，目录本身对 group 开放或
/// 是符号链接时拒绝。
#[test]
fn open_existing_checks_only_the_directory_itself() {
    let root = TempRoot::new();
    let shared = root.0.join("shared");
    fs::create_dir(&shared).expect("建立祖先目录");
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o770)).expect("设置权限");
    let receipts = shared.join("receipts");
    fs::create_dir(&receipts).expect("建立目录");
    fs::set_permissions(&receipts, fs::Permissions::from_mode(0o700)).expect("设置权限");
    let directory = PrivateDirectory::open_existing(&receipts).expect("祖先不在核对范围");
    directory.write("a.json", b"{}", false).expect("发布");

    fs::set_permissions(&receipts, fs::Permissions::from_mode(0o750)).expect("设置权限");
    assert!(PrivateDirectory::open_existing(&receipts).is_err(), "目录本身必须 owner-only");
    let link = shared.join("link");
    std::os::unix::fs::symlink(&receipts, &link).expect("创建符号链接");
    assert!(PrivateDirectory::open_existing(&link).is_err(), "拒绝符号链接");
    assert!(PrivateDirectory::open_existing(&shared.join("missing")).is_err());
}

/// 目录本身对 group 开放时拒绝，不放宽为可用的私有目录。
#[test]
fn non_private_directory_is_rejected() {
    let root = TempRoot::new();
    let open = root.0.join("open");
    fs::create_dir(&open).expect("建立目录");
    fs::set_permissions(&open, fs::Permissions::from_mode(0o750)).expect("设置权限");
    assert!(PrivateDirectory::prepare(&open).is_err());
    assert!(PrivateDirectory::prepare(&root.0.join("..").join("x")).is_err(), "拒绝 ..");
}
