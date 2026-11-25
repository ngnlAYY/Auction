//! 演练回执核对的核心安全边界：三种策略错配、被改写、早于最终实物、build 不符、缺人工签收。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use auction_http::{WebScriptPolicy, apply_browser_security_headers};
use auction_time::Timestamp;
use axum::http::{HeaderMap, header};
use serde_json::{Value, json};

use super::validation::{DIOXUS_CSP, SOLID_CSP};
use super::{Requirements, inspect, requirements_from_web_root};
use crate::support::digest::sha256_bytes;

const BUILD: &str = "solid-fixture-2";
const LEGACY: &str = "legacy-fixture-1";

struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).ok();
    }
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn at(text: &str) -> Timestamp {
    text.parse().expect("测试时间")
}

fn requirements(receipt_sha256: String) -> Requirements {
    Requirements {
        receipt_sha256,
        tag: "v9.9.9".to_owned(),
        source_commit: "a".repeat(40),
        archive_sha256: "b".repeat(64),
        build_id: BUILD.to_owned(),
        artifact_created_at: at("2026-11-24T08:00:00Z"),
        now: at("2026-11-24T12:00:00Z"),
    }
}

fn html(label: &str) -> String {
    sha256_bytes(label.as_bytes())
}

/// 一个响应观测；入口页的验证器按（端、策略）区分，与服务端“ETag 混入策略标识”一致。
fn response(phase: &str, surface: &str, kind: &str, status: u16, csp: &str, page: &str) -> Value {
    let pointer_build = if phase == "legacy" { LEGACY } else { BUILD };
    if kind == "api-error" {
        return json!({
            "phase": phase, "surface": surface, "kind": kind, "status": 404,
            "x_auction_build": pointer_build, "x_auction_build_independent": BUILD,
            "body_sha256": null, "csp_sha256": sha256_bytes(csp.as_bytes()),
            "etag": null, "cache_control": "no-store", "vary": null,
        });
    }
    json!({
        "phase": phase, "surface": surface, "kind": kind, "status": status,
        "x_auction_build": null, "x_auction_build_independent": null,
        "body_sha256": (status == 200).then(|| html(page)),
        "csp_sha256": sha256_bytes(csp.as_bytes()),
        "etag": format!("\"{surface}-{page}-{}\"", &sha256_bytes(csp.as_bytes())[..8]),
        "cache_control": (status == 200).then_some("no-cache"),
        "vary": (status == 200).then_some("Accept-Encoding"),
    })
}

fn receipt() -> Value {
    let mut responses = Vec::new();
    for surface in ["public", "admin"] {
        for phase in ["new", "legacy", "new-again"] {
            let (default_csp, default_page) =
                if phase == "legacy" { (DIOXUS_CSP, "legacy") } else { (SOLID_CSP, "solid") };
            for status in [200, 304] {
                responses.push(response(
                    phase,
                    surface,
                    "default-entry",
                    status,
                    default_csp,
                    default_page,
                ));
                responses.push(response(
                    phase,
                    surface,
                    "independent-entry",
                    status,
                    SOLID_CSP,
                    "solid",
                ));
            }
            responses.push(response(phase, surface, "api-error", 404, SOLID_CSP, "solid"));
        }
    }
    let surface = json!({
        "legacy_build_id": LEGACY, "legacy_entry_sha256": html("legacy"),
        "solid_build_id": BUILD, "solid_entry_sha256": html("solid"),
    });
    let passed = json!({ "result": "passed", "evidence": "browser-step.png" });
    json!({
        "format": "auction-web-switch-drill.v1",
        "drill_id": "drill-fixture", "operator": "operator", "reviewer": "reviewer",
        "started_at": "2026-11-24T09:00:00Z", "finished_at": "2026-11-24T10:00:00Z",
        "artifact": {
            "tag": "v9.9.9", "source_commit": "a".repeat(40),
            "archive_sha256": "b".repeat(64), "build_id": BUILD,
        },
        "surfaces": { "public": surface, "admin": surface },
        "responses": responses,
        "manual_signoff": {
            "wasm_started": passed,
            "legacy_interaction_state_changed": passed,
            "legacy_to_new_entry_read_ok": passed,
        },
        "anomalies": [],
        "conclusion": { "result": "passed", "summary": "演练通过" },
    })
}

/// 把回执写成文件并按它当时的摘要核对（等价于审核人签收后立即复验）。
fn check(receipt: &Value, adjust: impl FnOnce(&mut Requirements)) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(receipt).expect("序列化");
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let path = std::env::temp_dir().join(format!(
        "web-switch-drill-{}-{}.json",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, &bytes).expect("写回执");
    let _guard = TempFile(path.clone());
    let mut required = requirements(sha256_bytes(&bytes));
    adjust(&mut required);
    inspect(&path, &required).map(|_| ())
}

fn find_mut<'a>(receipt: &'a mut Value, phase: &str, kind: &str, status: u16) -> &'a mut Value {
    receipt["responses"]
        .as_array_mut()
        .expect("响应列表")
        .iter_mut()
        .find(|item| {
            item["surface"] == "public"
                && item["phase"] == phase
                && item["kind"] == kind
                && item["status"] == status
        })
        .expect("响应存在")
}

fn rejects_with(result: anyhow::Result<()>, needle: &str) {
    let error = result.expect_err("必须被拒绝");
    assert!(error.to_string().contains(needle), "错误应含“{needle}”，实际：{error}");
}

#[test]
fn a_faithful_receipt_for_the_final_artifact_is_accepted() {
    check(&receipt(), |_| {}).expect("忠实的回执应通过");
}

/// 三个错配各自只改一个响应的策略摘要，并且必须以“脚本策略与所属 build 不符”被拒
/// （不是被别的规则顺带拒掉）。旧 HTML 配 Solid 策略。
#[test]
fn legacy_html_with_solid_policy_is_rejected() {
    let mut mismatched = receipt();
    find_mut(&mut mismatched, "legacy", "default-entry", 200)["csp_sha256"] =
        json!(sha256_bytes(SOLID_CSP.as_bytes()));
    rejects_with(check(&mismatched, |_| {}), "脚本策略与所属 build 不符");
}

/// 回切期间新版独立入口配旧策略。
#[test]
fn solid_independent_entry_with_legacy_policy_is_rejected() {
    let mut mismatched = receipt();
    find_mut(&mut mismatched, "legacy", "independent-entry", 200)["csp_sha256"] =
        json!(sha256_bytes(DIOXUS_CSP.as_bytes()));
    rejects_with(check(&mismatched, |_| {}), "脚本策略与所属 build 不符");
}

/// CDN 重放：回切期间独立入口的 304 带着旧壳页面的策略（验证器仍回显 200 的）。
#[test]
fn cdn_replayed_304_with_the_other_policy_is_rejected() {
    let mut mismatched = receipt();
    find_mut(&mut mismatched, "legacy", "independent-entry", 304)["csp_sha256"] =
        json!(sha256_bytes(DIOXUS_CSP.as_bytes()));
    rejects_with(check(&mismatched, |_| {}), "脚本策略与所属 build 不符");
}

/// 回执文件在审核人签收之后被改写（即使改成“看起来正确”）必须被拒绝。
#[test]
fn a_receipt_rewritten_after_sign_off_is_rejected() {
    let signed = receipt();
    let signed_bytes = serde_json::to_vec_pretty(&signed).expect("序列化");
    let mut rewritten = signed;
    rewritten["drill_id"] = json!("drill-edited");
    let rewritten_bytes = serde_json::to_vec_pretty(&rewritten).expect("序列化");
    let path = std::env::temp_dir()
        .join(format!("web-switch-drill-rewritten-{}.json", std::process::id()));
    std::fs::write(&path, rewritten_bytes).expect("写回执");
    let _guard = TempFile(path.clone());
    let result = inspect(&path, &requirements(sha256_bytes(&signed_bytes))).map(|_| ());
    rejects_with(result, "已被改写");
}

/// 早于最终 tag 实物（演练的是旧制品）与 build ID 不符都拒绝。
#[test]
fn a_receipt_older_than_the_final_artifact_or_for_another_build_is_rejected() {
    rejects_with(
        check(&receipt(), |required| required.build_id = "solid-other".to_owned()),
        "build ID",
    );
}

/// 入口响应带了构建号头（服务端行为漂移）即拒绝；API 错误响应缺 no-store 同样拒绝。
#[test]
fn entry_build_headers_and_api_error_cache_control_are_checked() {
    for (kind, status) in [("default-entry", 200), ("independent-entry", 304)] {
        for header in ["x_auction_build", "x_auction_build_independent"] {
            let mut drifted = receipt();
            find_mut(&mut drifted, "new", kind, status)[header] = json!(BUILD);
            rejects_with(check(&drifted, |_| {}), "不应带构建号头");
        }
    }
    let mut cacheable = receipt();
    find_mut(&mut cacheable, "legacy", "api-error", 404)["cache_control"] = json!("no-cache");
    rejects_with(check(&cacheable, |_| {}), "API 错误响应");
}

/// 部署回执 `RECEIPT.v1` 的 `installed_at` 是实物时间下界：演练早于它即拒，晚于它通过；`issued_at`
/// 随每次 switch 刷新、可以晚于演练开始，不参与下界；
/// 全程经 `requirements_from_web_root`，调用方无法另给一个更早的参照时间。
#[test]
fn a_drill_older_than_the_web_root_receipt_installed_at_is_rejected() {
    let bytes = serde_json::to_vec_pretty(&receipt()).expect("序列化");
    let drill_path =
        std::env::temp_dir().join(format!("web-switch-drill-issued-{}.json", std::process::id()));
    std::fs::write(&drill_path, &bytes).expect("写回执");
    let _drill_guard = TempFile(drill_path.clone());
    let web_root =
        std::env::temp_dir().join(format!("web-switch-drill-root-{}", std::process::id()));
    std::fs::create_dir_all(&web_root).expect("建挂载根");
    let _root_guard = TempDir(web_root.clone());
    let run = |installed_at: &str| {
        let deployed = json!({
            "tag": "v9.9.9", "source_commit": "a".repeat(40), "archive_sha256": "b".repeat(64),
            "build_id": BUILD, "installed_at": installed_at,
            "issued_at": "2026-11-24T11:00:00Z",
        });
        std::fs::write(web_root.join("RECEIPT.v1"), serde_json::to_vec(&deployed).expect("序列化"))
            .expect("写部署回执");
        let required = requirements_from_web_root(
            &web_root,
            &sha256_bytes(&bytes),
            at("2026-11-24T12:00:00Z"),
        )?;
        inspect(&drill_path, &required).map(|_| ())
    };
    // 演练开始于 09:00。
    rejects_with(run("2026-11-24T09:30:00Z"), "早于最终 tag 实物");
    // issued_at 为 11:00（晚于演练开始），只要 installed_at 早于 09:00 仍通过。
    run("2026-11-24T08:00:00Z").expect("演练晚于 installed_at 应通过");
}

/// 浏览器步骤缺任一人工签收项即拒绝。
#[test]
fn a_missing_manual_sign_off_item_is_rejected() {
    let mut unsigned = receipt();
    unsigned["manual_signoff"]["legacy_interaction_state_changed"]["result"] = json!("not_run");
    rejects_with(check(&unsigned, |_| {}), "人工签收项");
}

/// 本地两条策略常量必须与服务端真实安全头逐字节相同，漂移时本测试失败。
#[test]
fn policy_constants_match_the_real_security_headers() {
    for (policy, expected) in
        [(WebScriptPolicy::Dioxus, DIOXUS_CSP), (WebScriptPolicy::Solid, SOLID_CSP)]
    {
        let mut headers = HeaderMap::new();
        apply_browser_security_headers(&mut headers, false, policy);
        assert_eq!(headers[header::CONTENT_SECURITY_POLICY], expected);
    }
}
