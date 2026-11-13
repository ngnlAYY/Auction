//! 追加字段必须保持父提交的 runtime canonical 摘要。
//!
//! 夹具由父提交 `7c5fe86d` 对 `{"operations":{"mail":{"managed":true}}}` 生成；
//! 这里固定 canonical bytes、fingerprint 与 request_hash，防止仅由新代码自证。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_application::admin::AdminRuntimeDocument;

const OLD_RAW: &str = r#"{"operations":{"mail":{"managed":true}}}"#;
const OLD_CANONICAL: &str = include_str!("fixtures/runtime_v1_mail.json");
const OLD_FINGERPRINT: &str =
    "sha256:8edc3e29dbd91d612816b629dbcfa8c40fe429db446642c3fc1890b5b5c34ee7";
const OLD_REQUEST_HASH: &str = "41f27ceb0ddd83475e1a06e24437fa6b5a1a407be9668c325af6e6f472c10f00";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn old_mail_document_keeps_parent_canonical_bytes_fingerprint_and_hash() {
    let document: AdminRuntimeDocument = serde_json::from_str(OLD_RAW).expect("old document");
    assert_eq!(serde_json::to_string(&document).expect("canonical JSON"), OLD_CANONICAL);
    assert_eq!(document.fingerprint(), OLD_FINGERPRINT);
    assert_eq!(
        hex(&document.request_hash(OLD_FINGERPRINT).expect("request hash").as_bytes()),
        OLD_REQUEST_HASH
    );
}

#[test]
fn changing_a_non_default_reply_budget_changes_the_fingerprint() {
    let mut document: AdminRuntimeDocument = serde_json::from_str(OLD_RAW).expect("old document");
    document
        .operations
        .as_mut()
        .expect("operations")
        .mail
        .as_mut()
        .expect("mail")
        .explanation_mail_limit = 11;
    assert_ne!(document.fingerprint(), OLD_FINGERPRINT);
}

/// 升级前由 Admin 写出的 runtime 文档：三个业务区块齐全，模板区块里是当时的默认正文
/// （卖家收款正文还没有装备名一行）。
const LEGACY_TEMPLATES_RAW: &str = r#"{
  "worlds": {"main": {"enabled": true}, "isekai": {"enabled": false}},
  "equipment_filter": {"enabled": false},
  "mail_templates": {
    "settlement_seller_payout": "Your auction payout is attached.\n\nAmount: {amount} Credits\n\nPlease collect the attached Credits.\n\nReference: {reference}",
    "auction_equipment_rejected": "Rejected without any reason placeholder."
  }
}"#;

#[test]
fn legacy_mail_template_section_is_inert_but_kept_for_older_binaries() {
    let document: AdminRuntimeDocument =
        serde_json::from_str(LEGACY_TEMPLATES_RAW).expect("legacy document");
    // 模板区块不再校验：其中的正文即使不满足现行占位符规则，runtime 文件也必须能加载。
    document.validate().expect("历史模板区块不影响文档校验");

    // 区块原样保留并写回：升级前的程序要求它与 worlds、equipment_filter 同时存在，
    // 新 Admin 保存后的文件仍要能被回退到的旧程序加载。
    let encoded = serde_json::to_value(&document).expect("canonical JSON");
    let payout = encoded["mail_templates"]["settlement_seller_payout"].as_str().unwrap_or_default();
    assert!(payout.starts_with("Your auction payout is attached."), "{payout}");
    assert!(!payout.contains("{item_name}"), "历史正文原样保留，不被默认正文替换：{payout}");
    assert_eq!(
        encoded["mail_templates"]["auction_equipment_rejected"],
        "Rejected without any reason placeholder."
    );

    // 指纹覆盖这个区块：同一份文件的指纹不因模板迁库而变化，改动它仍会被察觉。
    let mut edited = document.clone();
    edited.mail_templates = None;
    assert_ne!(document.fingerprint(), edited.fingerprint());
}

#[test]
fn new_documents_do_not_need_a_mail_template_section() {
    let document: AdminRuntimeDocument = serde_json::from_str(
        r#"{"worlds": {"main": {"enabled": true}, "isekai": {"enabled": false}},
            "equipment_filter": {"enabled": false}}"#,
    )
    .expect("new document");
    document.validate().expect("模板不再属于 runtime 文档的必需区块");
    assert!(
        serde_json::to_value(&document).expect("canonical JSON").get("mail_templates").is_none()
    );

    // worlds 与 equipment_filter 仍须成对出现。
    let unpaired: AdminRuntimeDocument = serde_json::from_str(
        r#"{"worlds": {"main": {"enabled": true}, "isekai": {"enabled": false}}}"#,
    )
    .expect("unpaired document");
    assert!(unpaired.validate().is_err());
}

#[test]
fn projection_for_untouched_documents_still_fills_all_three_business_sections() {
    // 从未写过业务区块的旧文档：投影补齐三个区块（模板区块只为旧程序能加载保存后的文件）。
    let mut document = AdminRuntimeDocument::default();
    document.materialize_business_defaults();
    assert!(document.worlds.is_some() && document.equipment_filter.is_some());
    assert!(document.mail_templates.is_some());
    document.validate().expect("补齐后的文档有效");
}
