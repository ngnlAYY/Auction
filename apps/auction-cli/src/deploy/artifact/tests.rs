use std::{
    fs,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use super::{CHECKSUMS, inspect, sha256_bytes};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("auction-artifact-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("建立临时制品目录");
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, relative: &str, bytes: &[u8], mode: u32) {
        let path = self.0.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("建立目录");
        }
        {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(mode)
                .open(&path)
                .expect("创建文件");
            std::io::Write::write_all(&mut file, bytes).expect("写入文件");
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("设置权限");
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn web_list(seed: &str) -> Vec<u8> {
    format!("{}  ./assets/app.js\n{}  ./index.html\n", "a".repeat(63) + seed, "b".repeat(63) + seed)
        .into_bytes()
}

/// 生成一个自洽的最小发布包；每个文件都进入清单与摘要（SHA256SUMS 自身除外）。
fn package() -> TempRoot {
    let root = TempRoot::new();
    let files: Vec<(&str, Vec<u8>, u32)> = vec![
        ("bin/auction-worker", b"worker".to_vec(), 0o755),
        ("bin/auction-admin-api", b"admin".to_vec(), 0o755),
        ("bin/auction-public-api", b"public".to_vec(), 0o755),
        ("bin/auction-cli", b"cli".to_vec(), 0o755),
    ];
    let public_list = web_list("1");
    let admin_list = web_list("2");
    let public_payload = sha256_bytes(&public_list);
    let admin_payload = sha256_bytes(&admin_list);
    let commit = "a".repeat(40);
    let fingerprint = "b".repeat(64);
    let mut entries = files.clone();
    entries.push(("docs/WEB-PUBLIC-SHA256", public_list, 0o644));
    entries.push(("docs/WEB-ADMIN-SHA256", admin_list, 0o644));
    entries.push((
        "docs/WEB-PUBLIC-MANIFEST.v1",
        format!(
            "schema=auction-web-assets/1\nsurface=public\nprofile=wasm-release\npayload_sha256={public_payload}\n"
        )
        .into_bytes(),
        0o644,
    ));
    entries.push((
        "docs/WEB-ADMIN-MANIFEST.v1",
        format!(
            "schema=auction-web-assets/1\nsurface=admin\nprofile=wasm-release\npayload_sha256={admin_payload}\n"
        )
        .into_bytes(),
        0o644,
    ));
    entries.push((
        "docs/DEPENDENCIES.md",
        format!("- Source commit：`{commit}`\n- Source worktree：`clean`\n").into_bytes(),
        0o644,
    ));
    for (relative, bytes, mode) in &entries {
        root.write(relative, bytes, *mode);
    }
    let mut manifest = format!(
        "schema=auction-release-manifest/1\npackage=auction-rust-0.1.0-{:12}\nsource_commit={commit}\nsource_fingerprint={fingerprint}\nfiles=\n",
        &commit[..12]
    );
    for (relative, _, mode) in &entries {
        manifest.push_str(&format!("{mode:o} {relative}\n"));
    }
    manifest.push_str("644 RELEASE-MANIFEST.v1\n");
    manifest.push_str(&format!("644 {CHECKSUMS}\n"));
    root.write("RELEASE-MANIFEST.v1", manifest.as_bytes(), 0o644);
    let mut sums = String::new();
    for (relative, bytes, _) in &entries {
        sums.push_str(&format!("{}  {relative}\n", sha256_bytes(bytes)));
    }
    sums.push_str(&format!("{}  RELEASE-MANIFEST.v1\n", sha256_bytes(manifest.as_bytes())));
    sums.push_str(&format!("{}  {CHECKSUMS}\n", "0".repeat(64)));
    root.write(CHECKSUMS, sums.as_bytes(), 0o644);
    root
}

#[test]
fn valid_release_package_passes() {
    let root = package();
    let artifact = inspect(root.path()).expect("自洽发布包必须通过");
    assert_eq!(artifact.source_commit, "a".repeat(40));
    assert_ne!(artifact.public_payload_sha256, artifact.admin_payload_sha256);
    assert_eq!(artifact.cli_sha256(), sha256_bytes(b"cli").as_str());
    assert!(artifact.binary("auction-worker").is_some());
}

#[test]
fn mutated_binary_or_missing_file_is_rejected() {
    let root = package();
    root.write("bin/auction-cli", b"tampered", 0o755);
    assert!(inspect(root.path()).is_err());

    let root = package();
    fs::remove_file(root.path().join("docs/WEB-ADMIN-SHA256")).expect("删除标记");
    assert!(inspect(root.path()).is_err());

    let root = package();
    root.write("docs/extra.md", b"extra", 0o644);
    assert!(inspect(root.path()).is_err());
}

#[test]
fn manifest_and_payload_inconsistencies_are_rejected() {
    let root = package();
    root.write("docs/WEB-PUBLIC-MANIFEST.v1", b"schema=auction-web-assets/1\nsurface=public\nprofile=wasm-release\npayload_sha256=0000000000000000000000000000000000000000000000000000000000000000\n", 0o644);
    assert!(inspect(root.path()).is_err());

    let root = package();
    root.write(
        "docs/DEPENDENCIES.md",
        format!("- Source commit：`{}`\n- Source worktree：`dirty`\n", "a".repeat(40)).as_bytes(),
        0o644,
    );
    // DEPENDENCIES 变更会同时改变摘要；这里必须重算 SHA256SUMS 才能命中工作树判定。
    let mut sums = String::new();
    for (relative, bytes, _) in members(root.path()) {
        sums.push_str(&format!("{}  {relative}\n", sha256_bytes(&bytes)));
    }
    sums.push_str(&format!("{}  {CHECKSUMS}\n", "0".repeat(64)));
    root.write(CHECKSUMS, sums.as_bytes(), 0o644);
    assert!(inspect(root.path()).is_err());
}

fn members(root: &Path) -> Vec<(String, Vec<u8>, u32)> {
    let mut result = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).expect("读取目录").flatten() {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).expect("读取元数据");
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            let relative =
                path.strip_prefix(root).expect("相对路径").to_str().expect("UTF-8").to_owned();
            if relative == CHECKSUMS {
                continue;
            }
            result.push((
                relative,
                fs::read(&path).expect("读取文件"),
                metadata.permissions().mode() & 0o777,
            ));
        }
    }
    result
}

#[test]
fn package_name_and_modes_are_enforced() {
    let root = package();
    let text = fs::read_to_string(root.path().join("RELEASE-MANIFEST.v1"))
        .expect("读取清单")
        .replace("auction-rust-0.1.0-", "auction-rust-0.1.0-single-");
    root.write("RELEASE-MANIFEST.v1", text.as_bytes(), 0o644);
    let mut sums = String::new();
    for (relative, bytes, _) in members(root.path()) {
        sums.push_str(&format!("{}  {relative}\n", sha256_bytes(&bytes)));
    }
    sums.push_str(&format!("{}  {CHECKSUMS}\n", "0".repeat(64)));
    root.write(CHECKSUMS, sums.as_bytes(), 0o644);
    assert!(inspect(root.path()).is_err());
}

#[test]
fn symlinks_are_rejected() {
    let root = package();
    std::os::unix::fs::symlink("/etc/hostname", root.path().join("docs/link"))
        .expect("建立符号链接");
    assert!(inspect(root.path()).is_err());
}
