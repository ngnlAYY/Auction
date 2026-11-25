//! 核对 Web 栈回执校验：双端 Solid 与程序摘要绑定才放行，其余失败关闭。

use serde_json::{Value, json};

use crate::{
    ConfigError, ConsignmentActivation, Role, WebStackBinary, WebStackFacts,
    consignment_activation_with_web_stack, validate_web_stack,
};

const BUILD: &str = "build-new";
const PUBLIC_SHA: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const ADMIN_SHA: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const WORKER_SHA: &str = "3333333333333333333333333333333333333333333333333333333333333333";

fn surface(build: &str) -> Value {
    json!({
        "stack": "solid",
        "payload_sha256": "a".repeat(64),
        "entry_build_id": build,
        "entry_script_policy": "solid",
        "legacy": { "build_id": "build-old", "payload_sha256": "b".repeat(64), "script_policy": "dioxus" }
    })
}

fn receipt() -> Value {
    json!({
        "schema": "auction-web-stack-receipt/1",
        "web_stack_id": "public-solid_admin-solid",
        "build_id": BUILD,
        "surfaces": { "public": surface(BUILD), "admin": surface(BUILD) },
        "binaries": {
            "auction-public-api": { "path": "/x", "sha256": PUBLIC_SHA },
            "auction-admin-api": { "path": "/x", "sha256": ADMIN_SHA },
            "auction-worker": { "path": "/x", "sha256": WORKER_SHA },
            "auction-server": null
        }
    })
}

fn check(
    bytes: Option<&[u8]>,
    role: Role,
    sha: Option<&str>,
    build: &str,
) -> Result<(), ConfigError> {
    validate_web_stack(&WebStackFacts {
        receipt_bytes: bytes,
        binary: role.into(),
        own_binary_sha256: sha,
        own_build_id: build,
    })
}

fn check_receipt(value: &Value) -> Result<(), ConfigError> {
    check(Some(value.to_string().as_bytes()), Role::Public, Some(PUBLIC_SHA), BUILD)
}

fn assert_rejected(result: Result<(), ConfigError>) {
    assert!(matches!(result, Err(ConfigError::ConsignmentWebStack(_))), "{result:?}");
}

#[test]
fn matching_receipt_is_accepted_for_every_role() {
    let bytes = receipt().to_string();
    for (role, sha) in
        [(Role::Public, PUBLIC_SHA), (Role::Admin, ADMIN_SHA), (Role::Worker, WORKER_SHA)]
    {
        check(Some(bytes.as_bytes()), role, Some(sha), BUILD).unwrap();
    }
}

#[test]
fn missing_or_garbled_receipt_is_rejected() {
    assert_rejected(check(None, Role::Public, Some(PUBLIC_SHA), BUILD));
    assert_rejected(check(Some(b"not json"), Role::Public, Some(PUBLIC_SHA), BUILD));
}

#[test]
fn single_surface_is_rejected() {
    let mut value = receipt();
    value["surfaces"].as_object_mut().unwrap().remove("admin");
    assert_rejected(check_receipt(&value));
}

#[test]
fn mixed_package_or_wrong_stack_is_rejected() {
    let mut value = receipt();
    value["surfaces"]["admin"]["entry_build_id"] = json!("build-other");
    assert_rejected(check_receipt(&value));

    let mut value = receipt();
    value["web_stack_id"] = json!("public-solid_admin-dioxus");
    assert_rejected(check_receipt(&value));

    let mut value = receipt();
    value["surfaces"]["public"]["stack"] = json!("dioxus");
    assert_rejected(check_receipt(&value));
}

#[test]
fn build_id_and_binary_digest_must_match_this_process() {
    let bytes = receipt().to_string();
    assert_rejected(check(Some(bytes.as_bytes()), Role::Public, Some(PUBLIC_SHA), "build-other"));
    assert_rejected(check(Some(bytes.as_bytes()), Role::Public, Some(ADMIN_SHA), BUILD));
    assert_rejected(check(Some(bytes.as_bytes()), Role::Public, None, BUILD));
}

#[test]
fn disabled_consignment_never_reads_the_receipt() {
    let facts = WebStackFacts {
        receipt_bytes: None,
        binary: WebStackBinary::Public,
        own_binary_sha256: None,
        own_build_id: BUILD,
    };
    assert_eq!(
        consignment_activation_with_web_stack(false, true, false, false, &facts).unwrap(),
        ConsignmentActivation::Disabled
    );
}

#[test]
fn enabled_consignment_requires_a_valid_receipt() {
    let bytes = receipt().to_string();
    let good = WebStackFacts {
        receipt_bytes: Some(bytes.as_bytes()),
        binary: WebStackBinary::Admin,
        own_binary_sha256: Some(ADMIN_SHA),
        own_build_id: BUILD,
    };
    assert_eq!(
        consignment_activation_with_web_stack(true, true, false, false, &good).unwrap(),
        ConsignmentActivation::Enabled
    );
    let missing = WebStackFacts { receipt_bytes: None, ..good };
    assert_rejected(
        consignment_activation_with_web_stack(true, true, false, false, &missing).map(|_| ()),
    );
    // 四事实不满足时先于回执报原有错误。
    assert!(matches!(
        consignment_activation_with_web_stack(true, false, false, false, &good),
        Err(ConfigError::ConsignmentPreviewRequired)
    ));
}

/// 回切旧壳后的一端：默认入口在登记的旧壳（dioxus），独立入口与顶层 build 仍是 Solid。
fn rolled_back(build: &str) -> Value {
    let mut value = surface(build);
    value["entry_build_id"] = json!("build-old");
    value["entry_script_policy"] = json!("dioxus");
    value
}

#[test]
fn rolled_back_receipt_with_registered_legacy_entry_is_accepted() {
    for (public, admin) in [
        (rolled_back(BUILD), surface(BUILD)),
        (surface(BUILD), rolled_back(BUILD)),
        (rolled_back(BUILD), rolled_back(BUILD)),
    ] {
        let mut value = receipt();
        value["surfaces"] = json!({ "public": public, "admin": admin });
        check_receipt(&value).unwrap();
    }
}

#[test]
fn rolled_back_receipt_with_any_other_entry_combination_is_rejected() {
    let reject = |mutate: &dyn Fn(&mut Value)| {
        let mut public = rolled_back(BUILD);
        mutate(&mut public);
        let mut value = receipt();
        value["surfaces"]["public"] = public;
        assert_rejected(check_receipt(&value));
    };
    // 入口指向未登记的 build。
    reject(&|surface| surface["entry_build_id"] = json!("build-unregistered"));
    // 新 build 配 dioxus 策略。
    reject(&|surface| surface["entry_build_id"] = json!(BUILD));
    // 旧壳 build 配 solid 策略。
    reject(&|surface| surface["entry_script_policy"] = json!("solid"));
    // 登记的旧壳策略不是 dioxus。
    reject(&|surface| surface["legacy"]["script_policy"] = json!("solid"));
    // 旧壳记录与本包 Solid build 相同：独立入口不能同时是旧壳。
    reject(&|surface| surface["legacy"]["build_id"] = json!(BUILD));
    // 入口与旧壳记录同指本包 Solid build 且策略标 dioxus：只靠显式的“旧壳不得是本包 build”判断拒绝。
    reject(&|surface| {
        surface["entry_build_id"] = json!(BUILD);
        surface["legacy"]["build_id"] = json!(BUILD);
    });
    // 独立入口不再是 Solid。
    reject(&|surface| surface["stack"] = json!("dioxus"));
}
