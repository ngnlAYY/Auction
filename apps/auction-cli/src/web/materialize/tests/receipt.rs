//! 页面栈回执与服务端挂载加载器的对接：写出的目录被 `auction-http` 的加载器完整接受，
//! 写出的 `RECEIPT.v1` 被启用校验接受，且栈、build 与程序摘要不符时被拒。

use std::collections::BTreeMap;
use std::fs;

use auction_config::{Role, WebStackFacts, validate_web_stack};
use auction_http::{SurfacePages, WebScriptPolicy, apply_browser_security_headers};
use auction_time::Timestamp;
use axum::http::HeaderMap;

use super::super::install::install_build;
use super::super::layout::{Stack, Surface};
use super::super::pointer::{SwitchTarget, init_pointer};
use super::super::receipt::{BinaryRecord, PackageRecord, render, write};
use super::super::verify::DIOXUS_CONTENT_SECURITY_POLICY;
use super::{Files, TempRoot, build_of_surface, old_files, overwrite, switch};
use crate::cli::SwitchArgs;

const BUILD: &str = "solid-2";

/// 本进程的构建号不会等于这里的任何 build，所以加载器必须真的读目录，不会退回内嵌页面。
#[derive(rust_embed::Embed)]
#[folder = "../../target/dist/api-only-assets/public"]
#[allow_missing = true]
struct NoEmbeddedPages;

fn solid_files() -> Files {
    vec![
        ("index.html".into(), b"<html>solid</html>".to_vec()),
        ("assets/index-1.js".into(), b"solid-js".to_vec()),
    ]
}

/// 两端各一个旧壳与同一个 Solid build，入口指向 Solid。
fn mounted_root() -> TempRoot {
    let root = TempRoot::new();
    for surface in [Surface::Public, Surface::Admin] {
        install_build(&root.0, &build_of_surface(surface, "legacy-1", Stack::Dioxus, &old_files()))
            .unwrap();
        install_build(&root.0, &build_of_surface(surface, BUILD, Stack::Solid, &solid_files()))
            .unwrap();
        init_pointer(&root.0.join(surface.as_str()), surface, BUILD).unwrap();
    }
    root
}

fn package() -> PackageRecord {
    let binary = |name: &str, digit: char| {
        (
            name.to_owned(),
            Some(BinaryRecord {
                path: format!("/opt/auction/bin/{name}"),
                sha256: digit.to_string().repeat(64),
            }),
        )
    };
    PackageRecord {
        package: "auction-rust-0.1.9-0123456789ab".into(),
        source_commit: "a".repeat(40),
        tag: "v0.1.9".into(),
        archive_sha256: "b".repeat(64),
        installed_at: "2026-11-24T08:00:00Z".parse().unwrap(),
        binaries: BTreeMap::from([
            binary("auction-public-api", '2'),
            binary("auction-admin-api", '3'),
            binary("auction-worker", '4'),
            binary("auction-cli", '5'),
            ("auction-server".to_owned(), None),
        ]),
    }
}

fn accepted(receipt: &[u8], role: Role, own: &str, build: &str) -> bool {
    let facts = WebStackFacts {
        receipt_bytes: Some(receipt),
        binary: role.into(),
        own_binary_sha256: Some(own),
        own_build_id: build,
    };
    validate_web_stack(&facts).is_ok()
}

fn receipt_bytes(root: &TempRoot) -> Vec<u8> {
    let value = render(&package(), &root.0, None, Timestamp::now()).unwrap();
    write(&root.0, &value).unwrap();
    fs::read(root.0.join("RECEIPT.v1")).unwrap()
}

/// install → switch old → switch new：`installed_at` 原样保留，`issued_at` 每次刷新；
/// 因此演练回执只要晚于 `installed_at`，即使早于最后一次 switch 的 `issued_at` 也成立。
#[test]
fn switch_keeps_installed_at_and_refreshes_issued_at() {
    let root = mounted_root();
    let installed = package().installed_at;
    write(&root.0, &render(&package(), &root.0, None, Timestamp::now()).unwrap()).unwrap();
    let mut issued = Vec::new();
    for (to, build) in [(SwitchTarget::Old, "legacy-1"), (SwitchTarget::New, BUILD)] {
        std::thread::sleep(std::time::Duration::from_millis(5));
        super::super::switch(&SwitchArgs {
            surface: Surface::Public,
            to,
            build_id: build.to_owned(),
            web_root: root.0.clone(),
        })
        .unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(root.0.join("RECEIPT.v1")).unwrap()).unwrap();
        let stamp = |field: &str| value[field].as_str().unwrap().parse::<Timestamp>().unwrap();
        assert_eq!(stamp("installed_at"), installed, "switch 不得刷新 installed_at");
        issued.push(stamp("issued_at"));
    }
    assert!(issued[0] < issued[1], "issued_at 每次重写都刷新");
    assert!(installed < issued[0]);
}

#[test]
fn written_directories_are_accepted_by_the_server_mount_loader() {
    let root = mounted_root();
    // 入口在 Solid：默认与独立入口是同一个 build。
    SurfacePages::with_public_mount::<NoEmbeddedPages>(WebScriptPolicy::Solid, &root.0).unwrap();
    SurfacePages::with_admin_mount::<NoEmbeddedPages>(WebScriptPolicy::Solid, &root.0).unwrap();
    // 回切旧壳：默认入口是 Dioxus 旧壳，独立入口仍是 Solid，两个 build 同时被加载。
    switch(&root.0, Surface::Public, SwitchTarget::Old, "legacy-1").unwrap();
    SurfacePages::with_public_mount::<NoEmbeddedPages>(WebScriptPolicy::Solid, &root.0).unwrap();

    // 反例：目录被改动，加载器同样失败关闭（与 CLI 自己的核对口径一致）。
    overwrite(&root.0.join("public/legacy-1/files/assets/app-1.js"), b"tampered");
    assert!(
        SurfacePages::with_public_mount::<NoEmbeddedPages>(WebScriptPolicy::Solid, &root.0)
            .is_err()
    );
}

#[test]
fn cli_dioxus_policy_is_byte_equal_to_the_server_policy() {
    let mut headers = HeaderMap::new();
    apply_browser_security_headers(&mut headers, false, WebScriptPolicy::Dioxus);
    assert_eq!(
        headers.get("content-security-policy").unwrap().to_str().unwrap(),
        DIOXUS_CONTENT_SECURITY_POLICY
    );
}

#[test]
fn written_receipt_is_accepted_by_the_activation_validator_only_for_the_matching_program() {
    let root = mounted_root();
    let receipt = receipt_bytes(&root);
    // 每个角色登记的摘要只对该角色的程序成立。
    assert!(accepted(&receipt, Role::Public, &"2".repeat(64), BUILD));
    assert!(accepted(&receipt, Role::Admin, &"3".repeat(64), BUILD));
    assert!(accepted(&receipt, Role::Worker, &"4".repeat(64), BUILD));
    // 程序摘要不符（旧回执配新程序）与 build 不符都被拒。
    assert!(!accepted(&receipt, Role::Public, &"3".repeat(64), BUILD));
    assert!(!accepted(&receipt, Role::Public, &"2".repeat(64), "solid-3"));
}

#[test]
fn receipt_records_the_actual_entry_so_a_registered_legacy_entry_is_accepted() {
    let root = mounted_root();
    assert!(accepted(&receipt_bytes(&root), Role::Public, &"2".repeat(64), BUILD));
    // 回切旧壳后重写的回执如实记录入口是登记的旧壳（dioxus），独立入口仍是 Solid：启用校验接受。
    switch(&root.0, Surface::Public, SwitchTarget::Old, "legacy-1").unwrap();
    let rolled_back = receipt_bytes(&root);
    assert!(accepted(&rolled_back, Role::Public, &"2".repeat(64), BUILD));
    // 错配回执被拒：旧壳 build 配 solid 策略。
    let mut value: serde_json::Value = serde_json::from_slice(&rolled_back).unwrap();
    value["surfaces"]["public"]["entry_script_policy"] = serde_json::json!("solid");
    let mismatched = serde_json::to_vec(&value).unwrap();
    assert!(!accepted(&mismatched, Role::Public, &"2".repeat(64), BUILD));
    // 换回 Solid 后重写的回执再次被接受。
    switch(&root.0, Surface::Public, SwitchTarget::New, BUILD).unwrap();
    assert!(accepted(&receipt_bytes(&root), Role::Public, &"2".repeat(64), BUILD));
}

#[test]
fn receipt_is_not_written_for_a_mixed_or_legacy_less_root() {
    // 两端 Solid build 不是同一个：不写回执。
    let root = mounted_root();
    install_build(
        &root.0,
        &build_of_surface(Surface::Admin, "solid-9", Stack::Solid, &solid_files()),
    )
    .unwrap();
    switch(&root.0, Surface::Admin, SwitchTarget::New, "solid-9").unwrap();
    assert!(render(&package(), &root.0, None, Timestamp::now()).is_err());

    // 缺旧壳：不写回执（没有回切记录，启用校验本也会拒绝）。
    let root = TempRoot::new();
    for surface in [Surface::Public, Surface::Admin] {
        install_build(&root.0, &build_of_surface(surface, BUILD, Stack::Solid, &solid_files()))
            .unwrap();
        init_pointer(&root.0.join(surface.as_str()), surface, BUILD).unwrap();
    }
    assert!(render(&package(), &root.0, None, Timestamp::now()).is_err());
}
