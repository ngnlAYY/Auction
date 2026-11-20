//! 从原始写回执验证宿主对首次尝试和较早未知尝试的裁定。

use std::{
    cell::RefCell,
    collections::BTreeSet,
    future::{Future, ready},
    pin::pin,
    rc::Rc,
    task::{Context, Poll, Waker},
};

use dioxus::prelude::{ReadableExt, ScopeId, VNode, VirtualDom};
use gloo_net::http::Method;
use serde::Deserialize;

use self::SharedReceiptRuling::{ReplayChecked, ThisAttempt, Unknown};
use super::{CommandHarness, rejections, test_page};
use crate::api::{
    AdminApiFailure, AdminWriteEndpoint, AdminWriteFailure, Rejection, WriteDispatchStub,
    WriteTransportStub, admin_fixture, send_stub_write, write_rejection_rules,
};

const ENTITY: &str = "write:fixture";
const TIMEOUT: &str = "request_timeout";

fn envelope(code: &str) -> String {
    serde_json::json!({
        "code": code, "message": "请求已拒绝", "data": null, "request_id": null,
    })
    .to_string()
}

fn assert_histories(failure: AdminWriteFailure, code: &str, settled: [bool; 2], case: &str) {
    for (prior_unknown, settled) in [false, true].into_iter().zip(settled) {
        let label = format!("{case}，已有未知={prior_unknown}");
        let control = Rc::new(CommandHarness::default());
        let mut dom = VirtualDom::new(test_page).with_root_context(control.clone());
        dom.rebuild_in_place();
        let commands = control.pending.get().unwrap();
        let attempts = Rc::new(RefCell::new(Vec::new()));
        let displayed = Rc::new(RefCell::new(Vec::new()));
        dom.in_scope(ScopeId::APP, || {
            let attempts = attempts.clone();
            let displayed = displayed.clone();
            let failure = failure.clone();
            assert!(commands.submit(
                ENTITY.to_owned(),
                "验证写入".to_owned(),
                move |command_id| {
                    let mut attempts = attempts.borrow_mut();
                    attempts.push(command_id);
                    let failure = if prior_unknown && attempts.len() == 1 {
                        AdminWriteFailure::outcome_unknown(TIMEOUT)
                    } else {
                        failure.clone()
                    };
                    ready(Err::<(), _>(failure))
                },
                |()| panic!("失败不能交付成功回执"),
                move |code| displayed.borrow_mut().push(code),
            ));
            assert!(commands.is_busy(ENTITY), "{label}");
        });
        dom.process_events();
        let command_id = attempts.borrow()[0];
        if prior_unknown {
            assert!(commands.contains(ENTITY), "{label}");
            assert!(!commands.is_busy(ENTITY), "{label}");
            assert_eq!(displayed.borrow().as_slice(), &[TIMEOUT], "{label}");
            assert!(rejections(commands).peek().is_empty(), "{label}");
            dom.in_scope(ScopeId::APP, || commands.host.retry(command_id));
            dom.process_events();
        }
        assert_eq!(
            attempts.borrow().as_slice(),
            vec![command_id; 1 + usize::from(prior_unknown)],
            "{label}"
        );
        let expected = if prior_unknown { vec![TIMEOUT, code] } else { vec![code] };
        assert_eq!(displayed.borrow().as_slice(), expected, "{label}");
        assert!(!commands.is_busy(ENTITY), "{label}");
        assert_eq!(commands.contains(ENTITY), !settled, "{label}");
        assert_eq!(*commands.source.peek().reload.peek(), u64::from(settled), "{label}");
        let notices = rejections(commands);
        let notices = notices.peek();
        if settled {
            assert_eq!(notices.len(), 1, "{label}");
            assert_eq!(notices[0].identity.command_id, command_id, "{label}");
            assert_eq!(notices[0].result.as_ref().err().map(String::as_str), Some(code), "{label}");
        } else {
            assert!(notices.is_empty(), "{label}");
        }
    }
}

fn assert_http_histories(
    endpoint: AdminWriteEndpoint,
    status: u16,
    body: impl AsRef<[u8]>,
    code: &str,
    settled: [bool; 2],
) {
    let case =
        format!("{endpoint:?} HTTP {status}，正文 {}", String::from_utf8_lossy(body.as_ref()));
    let failure = AdminWriteFailure::from_test_http(endpoint, status, body);
    assert_eq!(failure.observed_response_status(), Some(status), "{case}");
    assert_histories(failure, code, settled, &case);
}

#[derive(Deserialize)]
struct SharedReceiptSamples {
    histories: [String; 2],
    groups: Vec<SharedReceiptGroup>,
}

#[derive(Deserialize)]
struct SharedReceiptGroup {
    endpoints: Vec<String>,
    protocol_codes: Vec<String>,
    responses: Vec<SharedReceiptResponse>,
}

#[derive(Deserialize)]
struct SharedReceiptResponse {
    http_status: u16,
    ruling: SharedReceiptRuling,
    display_code: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum SharedReceiptRuling {
    Unknown,
    ThisAttempt,
    ReplayChecked,
}

struct SharedReceiptCase<'a> {
    endpoint: &'a str,
    code: &'a str,
    response: &'a SharedReceiptResponse,
}

fn expand_shared_receipts(groups: &[SharedReceiptGroup]) -> Vec<SharedReceiptCase<'_>> {
    let mut cases = Vec::new();
    for group in groups {
        assert!(!group.endpoints.is_empty());
        assert!(!group.protocol_codes.is_empty());
        assert!(!group.responses.is_empty());
        for endpoint in &group.endpoints {
            for code in &group.protocol_codes {
                for response in &group.responses {
                    cases.push(SharedReceiptCase { endpoint, code, response });
                }
            }
        }
    }
    cases
}

fn assert_sample_coverage(cases: &[SharedReceiptCase<'_>]) {
    let endpoints = AdminWriteEndpoint::SAMPLE_ENDPOINTS;
    let expected_codes: BTreeSet<_> =
        write_rejection_rules().map(|(code, _, _)| code).chain(["state_conflict"]).collect();
    let sample_codes: BTreeSet<_> = cases.iter().map(|case| case.code).collect();
    let missing: Vec<_> = expected_codes.difference(&sample_codes).collect();
    let extra: Vec<_> = sample_codes.difference(&expected_codes).collect();
    assert!(missing.is_empty() && extra.is_empty(), "协议码样本缺少 {missing:?}，多出 {extra:?}");
    let sample_endpoints: BTreeSet<_> = cases.iter().map(|case| case.endpoint).collect();
    assert_eq!(
        sample_endpoints,
        endpoints.iter().map(|&(label, _)| label).collect(),
        "样本须覆盖端点枚举的全部标签"
    );

    let replies: BTreeSet<_> = cases
        .iter()
        .map(|case| (case.code, case.response.http_status, case.response.ruling))
        .collect();
    for (code, status, scope) in write_rejection_rules() {
        match scope {
            Rejection::ThisAttempt => {
                assert!(
                    replies.contains(&(code, status, ThisAttempt)),
                    "样本缺少 {code} HTTP {status} 正例"
                );
                assert!(
                    cases.iter().any(|case| case.code == code
                        && case.response.http_status != status
                        && case.response.ruling == Unknown),
                    "样本缺少 {code} 的错误状态反例"
                );
            }
            Rejection::ReplayChecked => {
                assert_eq!(status, 409, "重放拒绝 {code} 必须匹配 HTTP 409");
                assert!(
                    replies.contains(&(code, 409, ReplayChecked)),
                    "样本缺少 {code} HTTP 409 正例"
                );
                for wrong in [400, 401, 403, 404, 422, 429, 500, 502, 503, 504] {
                    assert!(
                        replies.contains(&(code, wrong, Unknown)),
                        "样本缺少 {code} HTTP {wrong} 反例"
                    );
                }
            }
            Rejection::Unknown => panic!("生产码表中的 {code} 没有拒绝等级"),
        }
    }
    for &(endpoint, _) in endpoints {
        assert!(
            cases.iter().any(|case| case.endpoint == endpoint
                && case.code == "state_conflict"
                && case.response.http_status == 409),
            "样本缺少 {endpoint} HTTP 409 state_conflict"
        );
    }
}

fn shared_receipt_failure(
    dom: &VirtualDom,
    endpoint: AdminWriteEndpoint,
    status: u16,
    code: &str,
) -> AdminWriteFailure {
    const URL: &str = "/api/v1/admin/test-write";
    let body = envelope(code).into_bytes();
    dom.in_scope(ScopeId::APP, || {
        WriteTransportStub::install(
            || Ok("csrf-fixture".to_owned()),
            move |url, method, request_body, _timeout_millis| {
                assert_eq!(url, URL);
                assert_eq!(*method, Method::POST);
                assert_eq!(request_body, b"{}");
                WriteDispatchStub::OwnedResponse { status, body: body.clone() }
            },
        );
        let mut future = pin!(send_stub_write(URL, Method::POST, endpoint, "{}"));
        let Poll::Ready(Err(failure)) =
            future.as_mut().poll(&mut Context::from_waker(Waker::noop()))
        else {
            panic!("替身应直接返回 {endpoint:?} HTTP {status} {code} 的失败回执");
        };
        failure
    })
}

#[test]
fn shared_write_receipt_samples_match_pending_host_rulings() {
    let samples: SharedReceiptSamples =
        serde_json::from_str(admin_fixture!("write-receipt-rejections")).unwrap();
    assert_eq!(samples.histories, ["first_attempt", "prior_unknown"]);
    let cases = expand_shared_receipts(&samples.groups);
    assert_sample_coverage(&cases);

    let dom = VirtualDom::prebuilt(VNode::empty);
    for SharedReceiptCase { endpoint, code, response } in cases {
        let category = AdminWriteEndpoint::SAMPLE_ENDPOINTS
            .iter()
            .find_map(|&(label, category)| (label == endpoint).then_some(category))
            .unwrap();
        let status = response.http_status;
        let case = format!("{endpoint} HTTP {status} {code}");
        let failure = shared_receipt_failure(&dom, category, status, code);
        let (ruling, settled) = match response.ruling {
            SharedReceiptRuling::Unknown => (Rejection::Unknown, [false, false]),
            SharedReceiptRuling::ThisAttempt => (Rejection::ThisAttempt, [true, false]),
            SharedReceiptRuling::ReplayChecked => (Rejection::ReplayChecked, [true, true]),
        };
        assert_eq!(failure.observed_response_status(), Some(status), "{case}");
        assert_eq!(
            failure.display_code(),
            response.display_code.as_deref().unwrap_or(code),
            "{case}"
        );
        assert!(failure.classify() == ruling, "{case}，预期 {:?}", response.ruling);
        assert_histories(failure, response.display_code.as_deref().unwrap_or(code), settled, &case);
    }
}

#[test]
fn all_this_attempt_rejections_require_their_http_status_and_keep_prior_unknown() {
    // 根据契约与 F12 裁定固定期望；重放拒绝另用服务端样本覆盖。
    let cases: &[(u16, &[&str])] = &[
        (
            400,
            &[
                "invalid_request_id",
                "invalid_query",
                "invalid_json",
                "invalid_replay",
                "invalid_command_id",
                "invalid_auction_id",
                "invalid_auction_command",
                "invalid_auction_schedule",
                "invalid_auction_intake",
                "unsupported_equipment_source",
                "invalid_lot_id",
                "invalid_lot_action",
                "invalid_lot_command",
                "invalid_recovery_command",
                "invalid_world",
                "invalid_world_command",
                "invalid_delivery_id",
                "invalid_user_id",
                "invalid_user_command",
                "invalid_rename_id",
                "invalid_rename_command",
                "invalid_delivery_command",
                "invalid_alert_command",
                "invalid_credential_action",
                "invalid_credential_kind",
                "invalid_credential_label",
                "invalid_credential_player_name",
                "invalid_credential_request",
                "invalid_credential_secret",
                "invalid_credential_status",
                "invalid_credential_world",
                "invalid_mail_request",
                "invalid_runtime_config",
                "invalid_locale",
                "invalid_rule_document",
                "invalid_settlement_return_command",
                "invalid_settlement_return_id",
            ],
        ),
        (401, &["unauthenticated"]),
        (403, &["origin_forbidden", "csrf_invalid", "manual_early_settlement_disabled"]),
        (
            404,
            &[
                "not_found",
                "mail_template_not_found",
                "world_not_found",
                "delivery_not_found",
                "settlement_return_not_found",
            ],
        ),
        (405, &["method_not_allowed"]),
        (
            409,
            &[
                "command_conflict",
                "credential_conflict",
                "stale_credential_state",
                "rename_name_taken",
                "user_conflict",
                "stale_state_version",
                "stale_world_state",
                "world_has_active_auction",
                "auction_intake_unavailable",
                "seller_blacklisted",
                "equipment_conflict",
                "stale_recovery_state",
                "stale_delivery_state",
                "invalid_auction_state",
                "invalid_lot_state",
                "delivery_sent_sighted",
                "delivery_recipient_renamed",
                "delivery_command_snapshot_unreadable",
                "settlement_obligation_ineligible",
                "sent_proof_rejected",
                "settlement_return_action_conflict",
                "settlement_return_resend_limit",
                "settlement_return_obligation_ineligible",
                "settlement_return_send_unresolved",
                "settlement_return_observation_required",
                "recovery_send_unattributed",
                "recovery_not_confirmable",
                "recovery_evidence_unresolved",
                "external_access_paused",
                "poll_rejected",
            ],
        ),
        (413, &["body_too_large"]),
        (415, &["unsupported_media_type"]),
        (
            422,
            &[
                "invalid_mail_template",
                "invalid_runtime_switch",
                "not_acknowledgeable",
                "credential_login_rejected",
                "credential_login_challenge",
            ],
        ),
        (429, &["rate_limited", "credential_login_busy"]),
        (501, &["feature_not_supported"]),
        (503, &["credential_login_unavailable"]),
    ];
    for &(status, codes) in cases {
        for &code in codes {
            let body = envelope(code);
            assert_http_histories(AdminWriteEndpoint::General, status, &body, code, [true, false]);
            for (mismatch, fallback) in [(418, "http_418"), (500, "http_500")] {
                assert_http_histories(
                    AdminWriteEndpoint::General,
                    mismatch,
                    &body,
                    fallback,
                    [false, false],
                );
            }
        }
    }
}

#[test]
fn replay_rejections_clear_prior_unknown_only_on_verified_http_409() {
    for (code, body) in [
        ("alert_stale", admin_fixture!("error-alert-stale")),
        ("stale_auction_state", admin_fixture!("error-stale-auction-state")),
        ("stale_lot_state", admin_fixture!("error-stale-lot-state")),
        ("stale_user_state", admin_fixture!("error-stale-user-state")),
        ("rename_not_revertable", admin_fixture!("error-rename-not-revertable")),
        ("mail_template_conflict", admin_fixture!("error-mail-template-conflict")),
        ("runtime_switch_conflict", admin_fixture!("error-runtime-switch-conflict")),
        ("already_disposed", admin_fixture!("error-already-disposed")),
        ("stale_settlement_return_state", admin_fixture!("error-stale-settlement-return-state")),
        ("settlement_return_untrusted", admin_fixture!("error-settlement-return-untrusted")),
        ("settlement_return_unbound", admin_fixture!("error-settlement-return-unbound")),
        (
            "settlement_return_assets_mismatch",
            admin_fixture!("error-settlement-return-assets-mismatch"),
        ),
        (
            "settlement_return_proof_required",
            admin_fixture!("error-settlement-return-proof-required"),
        ),
        ("conflict", admin_fixture!("error-realtime-conflict")),
    ] {
        assert_http_histories(AdminWriteEndpoint::General, 409, body, code, [true, true]);
        for status in [400, 401, 403, 404, 422, 429, 500, 502, 503, 504] {
            let case = format!("HTTP {status} {code}");
            let failure =
                AdminWriteFailure::from_test_http(AdminWriteEndpoint::General, status, body);
            assert_eq!(failure.observed_response_status(), Some(status), "{case}");
            assert!(matches!(failure.classify(), Rejection::Unknown), "{case}");
            assert_histories(failure, &format!("http_{status}"), [false, false], &case);
        }
        // 写前读取可以收到同一份回执，写入状态仍然为空。
        let read_failure = AdminApiFailure::from_http_body(409, body);
        assert_eq!(read_failure.status, Some(409));
        let not_sent = AdminWriteFailure::not_sent(read_failure.code);
        assert_eq!(not_sent.observed_response_status(), None, "{code}");
        assert_histories(not_sent, code, [true, false], code);
        let unknown = AdminWriteFailure::outcome_unknown(code);
        assert_eq!(unknown.observed_response_status(), None, "{code}");
        assert_histories(unknown, code, [false, false], code);
    }
}

#[test]
fn state_conflict_requires_http_409_and_a_versioned_endpoint() {
    let body = envelope("state_conflict");
    for (endpoint, settles) in [
        (AdminWriteEndpoint::UserMutation, true),
        (AdminWriteEndpoint::RulesPublish, true),
        (AdminWriteEndpoint::AboutPublish, true),
        (AdminWriteEndpoint::General, false),
        (AdminWriteEndpoint::RenameRevert, false),
        (AdminWriteEndpoint::MailTemplate, false),
    ] {
        assert_http_histories(endpoint, 409, &body, "state_conflict", [settles; 2]);
        for (status, fallback) in [(400, "http_400"), (503, "http_503")] {
            let display = if settles { fallback } else { "state_conflict" };
            assert_http_histories(endpoint, status, &body, display, [false; 2]);
        }
    }
}

#[test]
fn errors_that_can_follow_a_committed_command_are_kept_pending() {
    for (status, code) in [
        (503, "storage_unavailable"),
        (409, "reconciliation_required"),
        (503, "outcome_unknown"),
        (500, "internal_error"),
        (401, "invalid_credentials"),
        (422, "new_error"),
    ] {
        assert_http_histories(
            AdminWriteEndpoint::General,
            status,
            envelope(code),
            code,
            [false; 2],
        );
    }
    for code in ["network_error", "invalid_response", "http_400", "http_413", "http_503"] {
        assert_histories(AdminWriteFailure::outcome_unknown(code), code, [false; 2], code);
    }
    for code in ["request_timeout", "invalid_request"] {
        assert_histories(AdminWriteFailure::not_sent(code), code, [true, false], code);
    }
}

#[test]
fn only_complete_413_can_reject_this_attempt() {
    let local = AdminWriteFailure::not_sent("body_too_large");
    assert_eq!(local.observed_response_status(), None);
    assert_histories(local, "body_too_large", [true, false], "发送前本地正文超限");
    assert_http_histories(
        AdminWriteEndpoint::MailTemplate,
        413,
        admin_fixture!("mail-template-body-too-large"),
        "body_too_large",
        [true, false],
    );
    assert_envelope_shape(413, "body_too_large", "http_413", [true, false]);
}

#[test]
fn malformed_409_cannot_prove_rejection() {
    assert_envelope_shape(409, "mail_template_conflict", "http_409", [true, true]);
}

fn assert_envelope_shape(status: u16, code: &str, fallback: &str, settled: [bool; 2]) {
    let endpoint = AdminWriteEndpoint::MailTemplate;
    assert_http_histories(endpoint, status, envelope(code), code, settled);
    let complete = serde_json::json!({
        "code": code, "message": "请求已拒绝", "data": {"revision": 6},
        "request_id": "admin-receipt",
    });
    assert_http_histories(endpoint, status, complete.to_string(), code, settled);
    for key in ["code", "message", "data", "request_id"] {
        let mut missing = complete.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert_http_histories(endpoint, status, missing.to_string(), fallback, [false; 2]);
    }
    for (key, value) in [
        ("code", serde_json::json!(409)),
        ("message", serde_json::json!(null)),
        ("data", serde_json::json!(["fixture"])),
        ("data", serde_json::json!(true)),
        ("request_id", serde_json::json!(true)),
        ("extra", serde_json::json!(null)),
    ] {
        let mut malformed = complete.clone();
        malformed[key] = value;
        assert_http_histories(endpoint, status, malformed.to_string(), fallback, [false; 2]);
    }
    let positional = serde_json::json!([code, "请求已拒绝", null, null]).to_string();
    for body in ["<html>错误</html>", "{", &positional] {
        assert_http_histories(endpoint, status, body, fallback, [false; 2]);
    }
}

#[test]
fn malformed_utf8_cannot_be_normalized_into_write_evidence() {
    let endpoint = AdminWriteEndpoint::MailTemplate;
    let malformed = b"{\"code\":\"mail_template_conflict\",\"message\":\"\xff\",\"data\":null,\"request_id\":null}";
    let valid = r#"{"code":"mail_template_conflict","message":"�","data":null,"request_id":null}"#;
    assert_http_histories(endpoint, 409, malformed, "http_409", [false; 2]);
    assert_http_histories(endpoint, 409, valid, "mail_template_conflict", [true; 2]);
}

#[test]
fn server_cannot_forge_internal_markers_or_verified_receipts() {
    // submit 的错误类型是 AdminWriteFailure。它的 VerifiedHttp 只接收名义类型
    // VerifiedWriteReceipt，字段与构造器均为 transport 私有。裸字符串、普通
    // AdminApiFailure 和外部同形结构体无法构造它；Clone 只能复制现有证据。
    for code in [
        "not_sent:alert_stale",
        "not_sent:mail_template_conflict",
        "write_endpoint:mail-template:409:mail_template_conflict",
        "write_endpoint:user-mutation:409:state_conflict",
        "invalid_request",
    ] {
        let failure =
            AdminWriteFailure::from_test_http(AdminWriteEndpoint::General, 409, envelope(code));
        assert!(matches!(&failure, AdminWriteFailure::OutcomeUnknown { .. }), "{code}");
        assert_eq!(failure.observed_response_status(), Some(409), "{code}");
        assert_histories(failure, "http_409", [false; 2], code);
    }
    assert_http_histories(
        AdminWriteEndpoint::General,
        409,
        envelope("alert_stale"),
        "alert_stale",
        [true; 2],
    );
}
