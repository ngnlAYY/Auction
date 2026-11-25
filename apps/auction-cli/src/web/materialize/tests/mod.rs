//! 静态物化的核心规则：旧字节只在全部核对通过后才落盘、build 目录要么完整要么不存在、
//! 入口指针原子切换且只指向核对过的 build、独立入口永远是 Solid。

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::thread;

use auction_integrations::WebFetchPolicy;
use url::Url;
use uuid::Uuid;

use super::export::read_export_tree;
use super::install::{InstallFile, Installed, NewBuild, install_build};
use super::layout::{BuildManifest, EntryPointer, MAX_MANIFEST_BYTES, Stack, Surface};
use super::materialize;
use super::payload::PayloadList;
use super::pointer::{SwitchTarget, init_pointer, read_pointer};
use super::pointer::{plan_switch, write_pointer};
use super::verify::{DIOXUS_CONTENT_SECURITY_POLICY, verify_build_dir};
use crate::support::digest::sha256_bytes;

mod receipt;

const DIOXUS_CSP: &str = DIOXUS_CONTENT_SECURITY_POLICY;
/// 另一栈（Solid）的 CSP，用来模拟“源站其实是 Solid 二进制”。
const SOLID_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'";

pub(super) struct TempRoot(pub(super) PathBuf);

impl TempRoot {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("auction-web-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        // 测试清理：封存的目录先放开权限再删。
        unseal(&self.0);
        fs::remove_dir_all(&self.0).unwrap();
    }
}

pub(super) fn unseal(path: &Path) {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir()) {
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        for entry in fs::read_dir(path).unwrap() {
            unseal(&entry.unwrap().path());
        }
    }
}

pub(super) type Files = Vec<(String, Vec<u8>)>;

pub(super) fn old_files() -> Files {
    vec![
        ("index.html".into(), b"<html>old shell</html>".to_vec()),
        ("assets/app-1.js".into(), b"console.log('old')".to_vec()),
        ("assets/app-1.js.br".into(), b"\x8b\x01brotli-entity".to_vec()),
        ("assets/app_bg-1.wasm".into(), b"\0asm-old".to_vec()),
    ]
}

pub(super) fn list_bytes(files: &Files) -> Vec<u8> {
    files
        .iter()
        .map(|(path, bytes)| format!("{}  ./{path}\n", sha256_bytes(bytes)))
        .collect::<String>()
        .into_bytes()
}

pub(super) fn list_of(files: &Files) -> PayloadList {
    PayloadList::parse(&list_bytes(files)).unwrap()
}

/// 回环替身：按 `(路径, Accept-Encoding)` 给出整段原始响应；未知路径走 `fallback`。
pub(super) fn serve(handler: impl Fn(&str, &str) -> Vec<u8> + Send + 'static) -> Url {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut head = Vec::new();
            let mut byte = [0_u8; 1];
            while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                head.push(byte[0]);
            }
            let head = String::from_utf8_lossy(&head).into_owned();
            let path = head.split_whitespace().nth(1).unwrap_or("").to_owned();
            let accept = head
                .lines()
                .find_map(|line| line.strip_prefix("accept-encoding: "))
                .unwrap_or("")
                .to_owned();
            // 写失败只意味着客户端已放弃该连接，不是被测行为。
            stream.write_all(&handler(&path, &accept)).ok();
        }
    });
    origin
}

pub(super) fn http(status: &str, extra: &[String], body: &[u8]) -> Vec<u8> {
    let mut out =
        format!("HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n", body.len());
    for header in extra {
        out.push_str(header);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    let mut bytes = out.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

/// 忠实的旧二进制：原文给 identity，`x.br` 的实体经 `Accept-Encoding: br` 取 `x` 得到。
pub(super) fn faithful(
    files: &Files,
    csp: &'static str,
) -> impl Fn(&str, &str) -> Vec<u8> + Send + 'static + use<> {
    let table: BTreeMap<String, Vec<u8>> = files.iter().cloned().collect();
    move |path, accept| {
        let relative = path.trim_start_matches('/');
        let mut headers = vec![format!("content-security-policy: {csp}")];
        let body = if accept == "br" {
            headers.push("content-encoding: br".to_owned());
            table.get(&format!("{relative}.br"))
        } else {
            table.get(relative)
        };
        match body {
            Some(body) => http("200 OK", &headers, body),
            None => http("404 Not Found", &[], b""),
        }
    }
}

/// 测试里模拟磁盘上的篡改：先放开只读权限再改写。
pub(super) fn overwrite(path: &Path, bytes: &[u8]) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
    fs::write(path, bytes).unwrap();
}

pub(super) fn surface_dir(web_root: &Path) -> PathBuf {
    web_root.join("public")
}

pub(super) fn staging_or_build_dirs(web_root: &Path) -> Vec<String> {
    match fs::read_dir(surface_dir(web_root)) {
        Ok(entries) => {
            entries.map(|entry| entry.unwrap().file_name().into_string().unwrap()).collect()
        }
        Err(_) => Vec::new(),
    }
}

type Handler = Box<dyn Fn(&str, &str) -> Vec<u8> + Send>;

async fn run_materialize(
    web_root: &Path,
    origin: &Url,
    list: &PayloadList,
) -> anyhow::Result<Installed> {
    run_materialize_with(web_root, origin, list, WebFetchPolicy::default()).await
}

async fn run_materialize_with(
    web_root: &Path,
    origin: &Url,
    list: &PayloadList,
    policy: WebFetchPolicy,
) -> anyhow::Result<Installed> {
    materialize(web_root, Surface::Public, "old-1", origin, list, policy).await
}

/// 与 `web switch` 的入口写入同一路径：先规划（核对两边 build），再原子写指针。
pub(super) fn switch(
    web_root: &Path,
    surface: Surface,
    to: SwitchTarget,
    build_id: &str,
) -> anyhow::Result<EntryPointer> {
    let next = plan_switch(web_root, surface, to, build_id)?;
    write_pointer(&web_root.join(surface.as_str()), &next)?;
    Ok(next)
}

#[tokio::test]
async fn materialize_installs_exact_bytes_as_a_sealed_build_and_leaves_the_pointer_alone() {
    let root = TempRoot::new();
    let files = old_files();
    let origin = serve(faithful(&files, DIOXUS_CSP));

    let outcome = run_materialize(&root.0, &origin, &list_of(&files)).await.unwrap();
    assert_eq!(outcome, Installed::Created);

    let build = surface_dir(&root.0).join("old-1");
    // 预压缩实体按未解压字节保存，路径与清单一致。
    assert_eq!(fs::read(build.join("files/assets/app-1.js.br")).unwrap(), files[2].1);
    assert_eq!(fs::read(build.join("files/index.html")).unwrap(), files[0].1);
    let mode = |path: PathBuf| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(build.join("files/index.html")), 0o444);
    assert_eq!(mode(build.join("files/assets")), 0o555);
    assert_eq!(mode(build.clone()), 0o555);
    let facts = verify_build_dir(&surface_dir(&root.0), Surface::Public, "old-1").unwrap();
    assert_eq!(facts.manifest.stack, Stack::Dioxus);
    assert!(read_pointer(&surface_dir(&root.0), Surface::Public).unwrap().is_none());
    // 再次物化同一 build 逐项一致，幂等。
    assert_eq!(
        run_materialize(&root.0, &origin, &list_of(&files)).await.unwrap(),
        Installed::AlreadyPresent
    );
}

#[tokio::test]
async fn materialize_installs_nothing_when_any_file_is_wrong() {
    let files = old_files();
    let spa_fallback = files[0].1.clone();
    let cases: Vec<(&str, Handler)> = vec![
        // 缺失的 chunk 被 SPA 回退成 200 入口页：状态码正确但字节不是清单里的那个。
        ("html fallback", {
            let inner = faithful(&files, DIOXUS_CSP);
            Box::new(move |path, accept| {
                if path.ends_with("app_bg-1.wasm") {
                    http(
                        "200 OK",
                        &[format!("content-security-policy: {DIOXUS_CSP}")],
                        &spa_fallback,
                    )
                } else {
                    inner(path, accept)
                }
            })
        }),
        // 同名异字节。
        ("same name different bytes", {
            let inner = faithful(&files, DIOXUS_CSP);
            Box::new(move |path, accept| {
                if path.ends_with("app-1.js") && accept != "br" {
                    http("200 OK", &[], b"console.log('tampered')")
                } else {
                    inner(path, accept)
                }
            })
        }),
        // 请求 br 却得到未编码字节（等价于被自动解压）。
        ("br not encoded", {
            let inner = faithful(&files, DIOXUS_CSP);
            Box::new(move |path, accept| {
                if path.ends_with("app-1.js") && accept == "br" {
                    http("200 OK", &[], b"decoded")
                } else {
                    inner(path, accept)
                }
            })
        }),
        // 漏一个旧 chunk。
        ("missing chunk", {
            let inner = faithful(&files, DIOXUS_CSP);
            Box::new(move |path, accept| {
                if path.ends_with("app_bg-1.wasm") {
                    http("404 Not Found", &[], b"")
                } else {
                    inner(path, accept)
                }
            })
        }),
        // 重定向不跟随。
        ("redirect", {
            let inner = faithful(&files, DIOXUS_CSP);
            Box::new(move |path, accept| {
                if path.ends_with("app-1.js") && accept != "br" {
                    http("302 Found", &["location: /index.html".to_owned()], b"")
                } else {
                    inner(path, accept)
                }
            })
        }),
        // Public 清单却连到 Admin 的源站：入口与共有路径的字节都是 Admin 的。
        ("admin origin", {
            let admin: Files = files
                .iter()
                .map(|(path, bytes)| {
                    let mut bytes = bytes.clone();
                    bytes.extend_from_slice(b"-admin");
                    (path.clone(), bytes)
                })
                .collect();
            Box::new(faithful(&admin, DIOXUS_CSP))
        }),
        // 源站其实是 Solid 二进制：入口页策略没有旧壳需要的两项。
        ("wrong origin policy", Box::new(faithful(&files, SOLID_CSP))),
    ];
    for (name, handler) in cases {
        let root = TempRoot::new();
        let origin = serve(handler);
        let result = run_materialize(&root.0, &origin, &list_of(&files)).await;
        assert!(result.is_err(), "{name} 必须失败");
        assert!(
            staging_or_build_dirs(&root.0).is_empty(),
            "{name} 不得留下任何目录：{:?}",
            staging_or_build_dirs(&root.0)
        );
    }
}

#[test]
fn payload_list_is_a_closed_safe_set() {
    let digest = "a".repeat(64);
    let index = format!("{digest}  ./index.html\n");
    for bad in [
        format!("{index}{digest}  ./../etc/passwd\n"),
        format!("{index}{digest}  ./a/./b.js\n"),
        format!("{index}{digest}  ./a.js\n{digest}  ./a.js\n"),
        format!("{index}{digest}  ./orphan.js.br\n"),
        format!("{index}{digest}  /abs.js\n"),
        format!("{digest}  ./a.js\n"),
        format!("{index}{}  ./a.js\n", "A".repeat(64)),
    ] {
        assert!(PayloadList::parse(bad.as_bytes()).is_err(), "{bad}");
    }
    assert!(
        PayloadList::parse(format!("{index}{digest}  ./a.js\n{digest}  ./a.js.br\n").as_bytes())
            .is_ok()
    );
}

#[test]
fn install_refuses_to_overwrite_a_build_with_different_content() {
    let root = TempRoot::new();
    let build = solid_build("new-1", "v1");
    assert_eq!(install_build(&root.0, &build).unwrap(), Installed::Created);
    assert_eq!(install_build(&root.0, &build).unwrap(), Installed::AlreadyPresent);
    let changed = solid_build("new-1", "v2");
    assert!(install_build(&root.0, &changed).is_err());
    // 原 build 未被改动。
    verify_build_dir(&surface_dir(&root.0), Surface::Public, "new-1").unwrap();
    assert_eq!(staging_or_build_dirs(&root.0), ["new-1"]);
}

pub(super) fn build_of(build_id: &str, stack: Stack, files: &Files) -> NewBuild {
    build_of_surface(Surface::Public, build_id, stack, files)
}

pub(super) fn build_of_surface(
    surface: Surface,
    build_id: &str,
    stack: Stack,
    files: &Files,
) -> NewBuild {
    let list = list_bytes(files);
    NewBuild {
        surface,
        build_id: build_id.to_owned(),
        stack,
        payload_sha256: sha256_bytes(&list),
        files: files
            .iter()
            .map(|(path, bytes)| InstallFile {
                path: path.clone(),
                sha256: sha256_bytes(bytes),
                brotli_entity: path.ends_with(".br"),
                bytes: bytes.clone(),
            })
            .collect(),
    }
}

pub(super) fn solid_build(build_id: &str, marker: &str) -> NewBuild {
    let files: Files = vec![
        ("index.html".into(), format!("<html>solid {marker}</html>").into_bytes()),
        ("assets/solid-1.js".into(), b"solid".to_vec()),
    ];
    build_of(build_id, Stack::Solid, &files)
}

pub(super) fn pointer_bytes(web_root: &Path) -> Vec<u8> {
    fs::read(surface_dir(web_root).join("ENTRY.v1")).unwrap()
}

pub(super) fn prepared() -> TempRoot {
    let root = TempRoot::new();
    install_build(&root.0, &build_of("old-1", Stack::Dioxus, &old_files())).unwrap();
    install_build(&root.0, &solid_build("new-1", "v1")).unwrap();
    assert!(init_pointer(&surface_dir(&root.0), Surface::Public, "new-1").unwrap());
    root
}

#[test]
fn switch_back_to_old_keeps_the_independent_entry_on_solid_and_is_reversible() {
    let root = prepared();
    let next = switch(&root.0, Surface::Public, SwitchTarget::Old, "old-1").unwrap();
    assert_eq!((next.build_id.as_str(), next.stack), ("old-1", Stack::Dioxus));
    assert_eq!(next.independent_build_id, "new-1");
    let on_disk = read_pointer(&surface_dir(&root.0), Surface::Public).unwrap().unwrap();
    assert_eq!(on_disk, next);
    assert!(pointer_bytes(&root.0).starts_with(b"schema=auction-web-entry/1\n"));
    // 回切后没有遗留临时文件。
    assert!(staging_or_build_dirs(&root.0).iter().all(|name| !name.contains("tmp")));

    let back = switch(&root.0, Surface::Public, SwitchTarget::New, "new-1").unwrap();
    assert_eq!((back.build_id.as_str(), back.stack), ("new-1", Stack::Solid));
    assert_eq!(back.independent_build_id, "new-1");
}

#[test]
fn switch_leaves_the_pointer_untouched_on_every_refusal() {
    let root = prepared();
    let before = pointer_bytes(&root.0);
    // 目标栈与 --to 不符、构建号不存在或带路径、指针目标被篡改，都不能换。
    assert!(switch(&root.0, Surface::Public, SwitchTarget::Old, "new-1").is_err());
    assert!(switch(&root.0, Surface::Public, SwitchTarget::New, "old-1").is_err());
    assert!(switch(&root.0, Surface::Public, SwitchTarget::Old, "missing").is_err());
    assert!(switch(&root.0, Surface::Public, SwitchTarget::Old, "../old-1").is_err());
    assert_eq!(pointer_bytes(&root.0), before);

    // 篡改旧壳里的一个文件：目标重新核对失败。
    let tampered = surface_dir(&root.0).join("old-1/files/assets/app-1.js");
    unseal(&surface_dir(&root.0).join("old-1"));
    overwrite(&tampered, b"evil");
    assert!(switch(&root.0, Surface::Public, SwitchTarget::Old, "old-1").is_err());
    assert_eq!(pointer_bytes(&root.0), before);
}

#[test]
fn switch_to_old_refuses_when_the_independent_build_changed_or_conflicts() {
    // 独立入口 build 被篡改：回切前重新核对两边，拒绝。
    let root = prepared();
    let before = pointer_bytes(&root.0);
    let independent = surface_dir(&root.0).join("new-1");
    unseal(&independent);
    overwrite(&independent.join("files/assets/solid-1.js"), b"evil");
    assert!(switch(&root.0, Surface::Public, SwitchTarget::Old, "old-1").is_err());
    assert_eq!(pointer_bytes(&root.0), before);

    // 两个 build 在同一 URL 上字节不同：无法无歧义分派，不换。
    let root = TempRoot::new();
    let mut conflicting = old_files();
    conflicting.push(("assets/solid-1.js".into(), b"old version of the same url".to_vec()));
    install_build(&root.0, &build_of("old-1", Stack::Dioxus, &conflicting)).unwrap();
    install_build(&root.0, &solid_build("new-1", "v1")).unwrap();
    init_pointer(&surface_dir(&root.0), Surface::Public, "new-1").unwrap();
    let before = pointer_bytes(&root.0);
    assert!(switch(&root.0, Surface::Public, SwitchTarget::Old, "old-1").is_err());
    assert_eq!(pointer_bytes(&root.0), before);
}

#[test]
fn init_pointer_never_replaces_an_existing_pointer() {
    let root = prepared();
    install_build(&root.0, &solid_build("new-2", "v2")).unwrap();
    let before = pointer_bytes(&root.0);
    assert!(!init_pointer(&surface_dir(&root.0), Surface::Public, "new-2").unwrap());
    assert_eq!(pointer_bytes(&root.0), before);
}

#[test]
fn export_tree_must_match_the_digest_list_byte_for_byte() {
    let root = TempRoot::new();
    let files = old_files();
    let tree = root.0.join("web-assets/public");
    for (path, bytes) in &files {
        let target = tree.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, bytes).unwrap();
    }
    let list = list_of(&files);
    let build =
        read_export_tree(&root.0, Surface::Public, &list, "new-1", &list.payload_sha256).unwrap();
    assert_eq!((build.stack, build.files.len()), (Stack::Solid, files.len()));
    // 包凭据里的摘要与清单不一致。
    assert!(read_export_tree(&root.0, Surface::Public, &list, "new-1", &"0".repeat(64)).is_err());
    // 导出树的一个字节被改。
    fs::write(tree.join("assets/app-1.js"), b"tampered").unwrap();
    assert!(
        read_export_tree(&root.0, Surface::Public, &list, "new-1", &list.payload_sha256).is_err()
    );
}

#[test]
fn on_disk_formats_match_the_server_loader_contract() {
    let manifest = build_of("b-1", Stack::Solid, &old_files()).manifest();
    assert_eq!(BuildManifest::parse(&manifest.render()).unwrap(), manifest);
    let pointer = EntryPointer {
        surface: Surface::Admin,
        build_id: "old-1".into(),
        stack: Stack::Dioxus,
        entry_sha256: "1".repeat(64),
        payload_sha256: "2".repeat(64),
        independent_build_id: "new-1".into(),
        independent_payload_sha256: "3".repeat(64),
    };
    let expected = format!(
        "schema=auction-web-entry/1\nsurface=admin\nbuild_id=old-1\nstack=dioxus\nscript_policy=dioxus\n\
entry_sha256={}\npayload_sha256={}\nindependent_build_id=new-1\nindependent_payload_sha256={}\n",
        "1".repeat(64),
        "2".repeat(64),
        "3".repeat(64)
    );
    assert_eq!(pointer.render(), expected);
    assert_eq!(EntryPointer::parse(&expected).unwrap(), pointer);
    // 乱序、缺行、script_policy 与 stack 不一致都拒绝。
    assert!(
        EntryPointer::parse(&expected.replace("script_policy=dioxus", "script_policy=solid"))
            .is_err()
    );
    assert!(
        EntryPointer::parse(&expected.replacen(
            "surface=admin\nbuild_id=old-1\n",
            "build_id=old-1\nsurface=admin\n",
            1
        ))
        .is_err()
    );
}

#[tokio::test]
async fn materialize_rejects_oversized_responses_without_installing() {
    let root = TempRoot::new();
    let files = old_files();
    let origin = serve(faithful(&files, DIOXUS_CSP));
    // 单文件上限小于最大的清单文件：任何一个超限都整体失败。
    let policy = WebFetchPolicy { max_file_bytes: 10, ..WebFetchPolicy::default() };
    assert!(run_materialize_with(&root.0, &origin, &list_of(&files), policy).await.is_err());
    assert!(staging_or_build_dirs(&root.0).is_empty());
}

#[test]
fn install_rejects_builds_over_the_loader_limits_before_writing() {
    let root = TempRoot::new();
    let mut files = old_files();
    for index in 0..4096 {
        files.push((format!("assets/extra-{index}.js"), b"x".to_vec()));
    }
    // 4 + 4096 个文件超过服务端挂载加载器的 4096 上限：装上去服务也无法启动。
    assert!(install_build(&root.0, &build_of("old-1", Stack::Dioxus, &files)).is_err());
    assert!(staging_or_build_dirs(&root.0).is_empty());
}

#[test]
fn install_rejects_a_manifest_over_the_loader_limit_and_leaves_no_trace() {
    let root = prepared();
    let before = pointer_bytes(&root.0);
    let entries_before = staging_or_build_dirs(&root.0);
    let mut files = old_files();
    // 文件数与字节都在上限内，但 4096 个长路径让清单渲染后超过 4 MiB。
    let prefix = ["a".repeat(250), "b".repeat(250), "c".repeat(250), "d".repeat(250)].join("/");
    for index in 0..4000 {
        files.push((format!("assets/{prefix}/extra-{index}.js"), b"x".to_vec()));
    }
    let build = build_of("old-2", Stack::Dioxus, &files);
    assert!(build.manifest().render().len() as u64 > MAX_MANIFEST_BYTES);
    let error = install_build(&root.0, &build).unwrap_err();
    assert!(format!("{error:#}").contains("清单超过上限"), "{error:#}");
    assert_eq!(staging_or_build_dirs(&root.0), entries_before);
    assert_eq!(pointer_bytes(&root.0), before);
}

#[test]
fn independent_entry_must_be_solid_when_the_pointer_is_created() {
    let root = TempRoot::new();
    install_build(&root.0, &build_of("old-1", Stack::Dioxus, &old_files())).unwrap();
    // 把旧壳当作独立入口建立指针：拒绝，且不写任何指针。
    assert!(init_pointer(&surface_dir(&root.0), Surface::Public, "old-1").is_err());
    assert!(!surface_dir(&root.0).join("ENTRY.v1").exists());
}
