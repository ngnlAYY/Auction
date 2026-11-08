//! W9.11 追加字段必须保持父提交的 runtime canonical 摘要。
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
