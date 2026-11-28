//! 新格式发布包核对的核心测试：栈与 profile 配对、混包、版本硬边界、摘要内外一致与导出树。
//! 每个用例先构造自洽的包，再只破坏一处并重算 SHA256SUMS，保证被拒的是页面凭据层而不是外层摘要。

use std::{
    fs,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use super::{CHECKSUMS, inspect_at};
use crate::support::digest::sha256_bytes;

/// 测试用的版本硬边界；与生产常量解耦，常量提升时测试不受影响。
const FIRST: &str = "0.1.9";
const COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const FINGERPRINT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

pub(super) struct TempRoot(PathBuf);

impl TempRoot {
    pub(super) fn path(&self) -> &Path {
        &self.0
    }

    pub(super) fn write(&self, relative: &str, bytes: &[u8], mode: u32) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().expect("父目录")).expect("建立目录");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(mode)
            .open(&path)
            .expect("创建文件");
        std::io::Write::write_all(&mut file, bytes).expect("写入文件");
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("设置权限");
    }

    fn read(&self, relative: &str) -> String {
        fs::read_to_string(self.0.join(relative)).expect("读取文件")
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// 包形态：版本、两端栈声明与两端页面标记的 profile，以及是否带页面凭据与导出树。
pub(super) struct Fixture {
    pub(super) version: &'static str,
    stack_file: bool,
    public_stack: &'static str,
    admin_stack: &'static str,
    public_profile: &'static str,
    admin_profile: &'static str,
}

impl Fixture {
    /// 新格式正式 Solid 包。
    pub(super) fn solid() -> Self {
        Self {
            version: "0.2.0",
            stack_file: true,
            public_stack: "solid",
            admin_stack: "solid",
            public_profile: "solid-release",
            admin_profile: "solid-release",
        }
    }

    /// 旧版本 Rust 包：没有 WEB-STACK.v1 与导出树，profile 为 wasm-release。
    pub(super) fn legacy() -> Self {
        Self {
            version: "0.1.0",
            stack_file: false,
            public_stack: "dioxus",
            admin_stack: "dioxus",
            public_profile: "wasm-release",
            admin_profile: "wasm-release",
        }
    }

    pub(super) fn package_name(&self) -> String {
        format!("auction-rust-{}-{}", self.version, &COMMIT[..12])
    }

    pub(super) fn build(&self) -> TempRoot {
        let root = TempRoot(
            std::env::temp_dir().join(format!("auction-web-stack-{}", uuid::Uuid::new_v4())),
        );
        for name in ["auction-worker", "auction-admin-api", "auction-public-api", "auction-cli"] {
            root.write(&format!("bin/{name}"), name.as_bytes(), 0o755);
        }
        root.write(
            "docs/DEPENDENCIES.md",
            format!("- Source commit：`{COMMIT}`\n- Source worktree：`clean`\n").as_bytes(),
            0o644,
        );
        let mut digests = Vec::new();
        for (surface, profile) in [("public", self.public_profile), ("admin", self.admin_profile)] {
            let files = payload(surface);
            let mut listing = String::new();
            for (path, bytes) in &files {
                listing.push_str(&format!("{}  ./{path}\n", sha256_bytes(bytes)));
                if self.stack_file {
                    root.write(&format!("web-assets/{surface}/{path}"), bytes, 0o644);
                }
            }
            let digest = sha256_bytes(listing.as_bytes());
            let upper = surface.to_uppercase();
            root.write(&format!("docs/WEB-{upper}-SHA256"), listing.as_bytes(), 0o644);
            root.write(
                &format!("docs/WEB-{upper}-MANIFEST.v1"),
                format!(
                    "schema=auction-web-assets/1\nsurface={surface}\nprofile={profile}\npayload_sha256={digest}\n"
                )
                .as_bytes(),
                0o644,
            );
            digests.push(digest);
        }
        if self.stack_file {
            root.write(
                "docs/WEB-STACK.v1",
                format!(
                    "schema=auction-web-stack/1\nsource_commit={COMMIT}\nsource_fingerprint={FINGERPRINT}\npublic_stack={}\nadmin_stack={}\npublic_payload_sha256={}\nadmin_payload_sha256={}\nbuild_id=build-1\n",
                    self.public_stack, self.admin_stack, digests[0], digests[1]
                )
                .as_bytes(),
                0o644,
            );
        }
        let header = format!(
            "schema=auction-release-manifest/1\npackage={}\nsource_commit={COMMIT}\nsource_fingerprint={FINGERPRINT}\nfiles=\n",
            self.package_name()
        );
        seal(&root, &header);
        root
    }
}

/// 一个 surface 的页面文件；两端字节互不相同，使摘要清单不会相等。
fn payload(surface: &str) -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("assets/app.js", format!("js-{surface}").into_bytes()),
        ("assets/app.js.br", format!("br-{surface}").into_bytes()),
        ("index.html", format!("html-{surface}").into_bytes()),
    ]
}

fn collect(root: &Path, directory: &Path, out: &mut Vec<(String, u32)>) {
    for entry in fs::read_dir(directory).expect("读取目录").flatten() {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).expect("读取元数据");
        if metadata.is_dir() {
            collect(root, &path, out);
        } else {
            let relative = path.strip_prefix(root).expect("相对路径").to_str().expect("UTF-8");
            out.push((relative.to_owned(), metadata.permissions().mode() & 0o777));
        }
    }
}

/// 按目录现状重写 RELEASE-MANIFEST.v1 与 SHA256SUMS，使外层摘要层总是自洽。
fn seal(root: &TempRoot, header: &str) {
    let mut members = Vec::new();
    collect(root.path(), root.path(), &mut members);
    members.retain(|(path, _)| path != "RELEASE-MANIFEST.v1" && path != CHECKSUMS);
    members.sort();
    let mut manifest = header.to_owned();
    for (path, mode) in &members {
        manifest.push_str(&format!("{mode:o} {path}\n"));
    }
    manifest.push_str("644 RELEASE-MANIFEST.v1\n644 SHA256SUMS\n");
    root.write("RELEASE-MANIFEST.v1", manifest.as_bytes(), 0o644);
    let mut sums = String::new();
    for (path, _) in &members {
        sums.push_str(&format!(
            "{}  {path}\n",
            sha256_bytes(&fs::read(root.path().join(path)).expect("读取"))
        ));
    }
    sums.push_str(&format!("{}  RELEASE-MANIFEST.v1\n", sha256_bytes(manifest.as_bytes())));
    root.write(CHECKSUMS, sums.as_bytes(), 0o644);
}

/// 破坏之后按现有清单头重新封装。
fn reseal(root: &TempRoot) {
    let text = root.read("RELEASE-MANIFEST.v1");
    let end = text.find("files=\n").expect("files 段") + "files=\n".len();
    seal(root, &text[..end]);
}

fn rejected(root: &TempRoot, expected: &str) {
    let error = inspect_at(root.path(), FIRST).expect_err("必须拒绝");
    let text = format!("{error:#}");
    assert!(text.contains(expected), "拒绝原因应含“{expected}”，实际：{text}");
}

#[test]
fn solid_package_passes_and_legacy_rust_package_passes_in_old_format() {
    let root = Fixture::solid().build();
    let artifact = inspect_at(root.path(), FIRST).expect("新格式 Solid 包必须通过");
    let identity = artifact.web_stack.expect("新格式带页面凭据");
    assert_eq!(identity.id(), "public-solid_admin-solid");
    assert_eq!(identity.build_id, "build-1");
    assert_eq!(identity.public_payload_sha256, artifact.public_payload_sha256);
    assert_eq!(identity.admin_payload_sha256, artifact.admin_payload_sha256);

    // 回归：旧版本的 Rust 包按旧格式通过，且不带页面凭据。
    let legacy = Fixture::legacy().build();
    let artifact = inspect_at(legacy.path(), FIRST).expect("旧版本 Rust 包必须按旧格式通过");
    assert!(artifact.web_stack.is_none());
}

#[test]
fn missing_web_stack_is_rejected_from_the_boundary_version_on() {
    for version in ["0.1.9", "0.2.0"] {
        let root = Fixture { stack_file: false, version, ..Fixture::solid() }.build();
        // 没有页面凭据的新版本包，即使页面标记自洽（Solid profile）也拒绝。
        rejected(&root, "缺少 docs/WEB-STACK.v1");
    }
    let root = Fixture::solid().build();
    fs::remove_file(root.path().join("docs/WEB-STACK.v1")).expect("删除页面凭据");
    reseal(&root);
    rejected(&root, "缺少 docs/WEB-STACK.v1");
}

#[test]
fn legacy_package_must_not_carry_an_export_tree() {
    let root = Fixture::legacy().build();
    root.write("web-assets/public/index.html", b"html-public", 0o644);
    reseal(&root);
    rejected(&root, "旧格式发布包不得携带页面导出树");
}

#[test]
fn stack_and_profile_must_agree() {
    // solid 栈配 wasm-release 标记。
    let root = Fixture { public_profile: "wasm-release", ..Fixture::solid() }.build();
    rejected(&root, "Web 资源标记的 schema、surface 或部署 profile 无效：public");
    // 旧格式配 solid-release 标记（栈未声明却换了 profile）。
    let root = Fixture { public_profile: "solid-release", ..Fixture::legacy() }.build();
    rejected(&root, "Web 资源标记的 schema、surface 或部署 profile 无效：public");
}

#[test]
fn mixed_or_non_solid_stacks_are_rejected() {
    let root = Fixture { admin_stack: "dioxus", admin_profile: "wasm-release", ..Fixture::solid() }
        .build();
    rejected(&root, "页面栈混用");
    let root = Fixture {
        public_stack: "dioxus",
        admin_stack: "dioxus",
        public_profile: "wasm-release",
        admin_profile: "wasm-release",
        ..Fixture::solid()
    }
    .build();
    rejected(&root, "必须均为 solid");
}

#[test]
fn digest_must_agree_inside_and_outside() {
    // WEB-STACK.v1 里的摘要被改动。
    let root = Fixture::solid().build();
    let text = root.read("docs/WEB-STACK.v1");
    let line = text.lines().find(|line| line.starts_with("public_payload_sha256=")).expect("字段");
    root.write(
        "docs/WEB-STACK.v1",
        text.replace(line, &format!("public_payload_sha256={}", "0".repeat(64))).as_bytes(),
        0o644,
    );
    reseal(&root);
    rejected(&root, "WEB-STACK.v1 的 public 摘要与 WEB-PUBLIC-SHA256 不一致");

    // 页面标记里的摘要被改动，WEB-STACK.v1 保持原样。
    let root = Fixture::solid().build();
    let text = root.read("docs/WEB-ADMIN-MANIFEST.v1");
    let digest =
        text.lines().last().expect("末行").trim_start_matches("payload_sha256=").to_owned();
    root.write(
        "docs/WEB-ADMIN-MANIFEST.v1",
        text.replace(&digest, &"0".repeat(64)).as_bytes(),
        0o644,
    );
    reseal(&root);
    rejected(&root, "payload_sha256 与摘要清单不一致");

    // 来源与清单头不一致。
    let root = Fixture::solid().build();
    let text = root.read("docs/WEB-STACK.v1");
    root.write(
        "docs/WEB-STACK.v1",
        text.replace(
            &format!("source_commit={COMMIT}"),
            &format!("source_commit={}", "c".repeat(40)),
        )
        .as_bytes(),
        0o644,
    );
    reseal(&root);
    rejected(&root, "来源与 RELEASE-MANIFEST.v1 不一致");
}

#[test]
fn web_stack_file_must_be_closed_and_ordered() {
    for mutate in [
        |text: &str| text.replace("build_id=build-1\n", "build_id=build-1\nextra=1\n"),
        |text: &str| text.replace("build_id=build-1\n", "\nbuild_id=build-1\n"),
        |text: &str| text.replace("build_id=build-1\n", "build_id=bad id\n"),
        // 与 web 安装口径一致：以 `.` 开头的 build_id 不能通过制品校验。
        |text: &str| text.replace("build_id=build-1\n", "build_id=.hidden\n"),
        |text: &str| text.trim_end_matches('\n').to_owned(),
    ] {
        let root = Fixture::solid().build();
        let text = root.read("docs/WEB-STACK.v1");
        root.write("docs/WEB-STACK.v1", mutate(&text).as_bytes(), 0o644);
        reseal(&root);
        assert!(inspect_at(root.path(), FIRST).is_err());
    }
}

#[test]
fn export_tree_must_match_the_digest_listing_exactly() {
    // 少文件。
    let root = Fixture::solid().build();
    fs::remove_file(root.path().join("web-assets/public/assets/app.js.br")).expect("删除");
    reseal(&root);
    rejected(&root, "导出树与摘要清单的文件集合不一致");

    // 多文件。
    let root = Fixture::solid().build();
    root.write("web-assets/admin/assets/unlisted.js", b"x", 0o644);
    reseal(&root);
    rejected(&root, "导出树与摘要清单的文件集合不一致");

    // 同名异字节：路径集合不变，字节被换。
    let root = Fixture::solid().build();
    root.write("web-assets/public/index.html", b"tampered", 0o644);
    reseal(&root);
    rejected(&root, "导出文件字节与摘要清单不一致");

    // Public 混入 Admin 的文件（同路径，Admin 字节）与第三棵树。
    let root = Fixture::solid().build();
    let admin = fs::read(root.path().join("web-assets/admin/index.html")).expect("读取");
    root.write("web-assets/public/index.html", &admin, 0o644);
    reseal(&root);
    rejected(&root, "导出文件字节与摘要清单不一致");
    let root = Fixture::solid().build();
    root.write("web-assets/other/index.html", b"x", 0o644);
    reseal(&root);
    rejected(&root, "只允许 public 与 admin 两棵导出树");

    // 权限不是 0644。
    let root = Fixture::solid().build();
    root.write("web-assets/public/index.html", b"html-public", 0o600);
    reseal(&root);
    rejected(&root, "导出文件必须为 0644");
}
