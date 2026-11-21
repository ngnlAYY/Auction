//! 登记、改价和取消的 HTTP 守卫、输入边界和账本结果。

use std::sync::Arc;

use auction_application::consignment::{ConsignmentError, ListingBusinessRejection as Rejection};
use auction_contracts::PUBLIC_EXPECTED_USER_ID_HEADER;
use auction_domain::consignment::{FeeBps, ListingMode};
use auction_domain::{CommandId, UserId};
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderValue, Method, Request, header};
use serde_json::Value;

use crate::public::consignment::tests::fixtures::{
    Backend, Call, EXPIRED_COOKIE, FAKE_LINK, GOOD_ORIGIN, LISTING, LISTINGS, OTHER_SELLER, SELLER,
    Session, Wire, WriteOutcome, configured, fixture_uuid, login, register_service, request,
    router, send, setup,
};
use crate::server::contract_fixtures::{assert_wire, wire_fixture};

const MEDIA_JSON: &str = "application/json";
const FIRST_REQUEST_ID: &str = "P3A.First_1";
const REPLAY_REQUEST_ID: &str = "P3A.Replay-2";

#[derive(Clone, Copy, Debug)]
enum Command {
    Register,
    Reprice,
    Withdraw,
}

impl Command {
    fn method(self) -> Method {
        match self {
            Self::Register | Self::Withdraw => Method::POST,
            Self::Reprice => Method::PATCH,
        }
    }

    fn uri(self) -> String {
        match self {
            Self::Register => LISTINGS.to_owned(),
            Self::Reprice => format!("{LISTINGS}/{LISTING}/price"),
            Self::Withdraw => format!("{LISTINGS}/{LISTING}/withdraw"),
        }
    }

    fn input(self) -> Value {
        let fixture = match self {
            Self::Register => "public/consignment-register-request",
            Self::Reprice => "public/consignment-reprice-request",
            Self::Withdraw => "public/consignment-withdraw-request",
        };
        let mut input = wire_fixture(fixture);
        if matches!(self, Self::Register) {
            input["equipment_url"] = Value::from(FAKE_LINK);
        }
        input
    }

    fn request(self) -> Request<Body> {
        let mut req = self.request_with(&self.uri(), self.input().to_string());
        login(&mut req, SELLER);
        req
    }

    fn request_with(self, uri: &str, body: String) -> Request<Body> {
        let mut req = request(self.method(), uri, Body::from(body));
        req.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(MEDIA_JSON));
        req.headers_mut().insert(header::ORIGIN, HeaderValue::from_static(GOOD_ORIGIN));
        req
    }

    async fn send(self, app: &Router) -> Wire {
        send(app, self.request()).await
    }
}

const COMMANDS: [Command; 3] = [Command::Register, Command::Reprice, Command::Withdraw];

fn set_outcome(backend: &Backend, outcome: WriteOutcome) {
    *backend.command_outcome.lock().expect("命令结果") = outcome;
}

#[tokio::test]
async fn mutations_keep_first_response_on_same_request_replay_even_after_disable() {
    for (command, status, code, fixture) in [
        (Command::Register, 202, "accepted", "public/consignment-register-receipt"),
        (Command::Reprice, 200, "ok", "public/consignment-reprice-receipt"),
        (Command::Withdraw, 200, "ok", "public/consignment-withdraw-receipt"),
    ] {
        let backend = Arc::new(Backend::default());
        let on = router(setup(true, &backend));
        let mut request = command.request();
        request.headers_mut().insert("x-request-id", HeaderValue::from_static(FIRST_REQUEST_ID));
        let first = send(&on, request).await;
        first.assert(status, code);
        assert_wire(fixture, &first.body);
        assert_eq!(backend.take_request_ids(), vec![Some(FIRST_REQUEST_ID.to_owned())]);

        let seller = UserId::from_uuid(SELLER);
        let first_calls = backend.take_calls();
        match command {
            Command::Register => assert_eq!(
                first_calls,
                vec![
                    Call::ValidateUrl,
                    Call::Register(
                        seller,
                        CommandId::from_uuid(fixture_uuid(0x65)),
                        ListingMode::Consigned,
                        101,
                        500,
                        true,
                        500
                    ),
                ]
            ),
            Command::Reprice => assert_eq!(
                first_calls,
                vec![Call::Reprice(
                    seller,
                    CommandId::from_uuid(fixture_uuid(0x66)),
                    LISTING,
                    1,
                    102,
                    true,
                )]
            ),
            Command::Withdraw => assert_eq!(
                first_calls,
                vec![Call::Withdraw(seller, CommandId::from_uuid(fixture_uuid(0x68)), LISTING, 2,)]
            ),
        }
        set_outcome(&backend, WriteOutcome::Replay);
        for request_id in [FIRST_REQUEST_ID, REPLAY_REQUEST_ID] {
            let mut request = command.request();
            request.headers_mut().insert("x-request-id", HeaderValue::from_static(request_id));
            let replay = send(&on, request).await;
            replay.assert(status, code);
            assert_eq!(replay.body, first.body);
            assert_eq!(backend.take_calls(), first_calls);
            assert_eq!(backend.take_request_ids(), vec![Some(request_id.to_owned())]);
        }

        let mut off = setup(false, &backend);
        off.capabilities.registration_sealer_available = false;
        let replay = command.send(&router(off)).await;
        replay.assert(status, code);
        assert_eq!(replay.body, first.body);
        let calls = backend.take_calls();
        match command {
            Command::Register => assert!(matches!(
                calls.as_slice(),
                [Call::ValidateUrl, Call::Register(_, _, _, _, 500, false, 500)]
            )),
            Command::Reprice => {
                assert!(matches!(calls.as_slice(), [Call::Reprice(_, _, _, _, _, false)]));
            }
            Command::Withdraw => assert_eq!(calls, first_calls),
        }
    }
}

#[tokio::test]
async fn changing_fee_never_preempts_registration_arbitration() {
    let backend = Arc::new(Backend::default());
    let mut state = setup(true, &backend);
    let changed_fee = FeeBps::new(615).expect("新费率");
    state.capabilities.fee_bps = changed_fee;
    state.capabilities.register = Some(register_service(&backend, true, changed_fee));
    let app = router(state);
    for (mode, expected_mode) in
        [("consigned", ListingMode::Consigned), ("direct", ListingMode::Direct)]
    {
        let command = Command::Register;
        let mut input = command.input();
        input["mode"] = Value::from(mode);
        let register = || {
            let mut req = command.request_with(LISTINGS, input.to_string());
            login(&mut req, SELLER);
            req
        };
        set_outcome(&backend, WriteOutcome::Replay);
        send(&app, register()).await.assert(202, "accepted");
        assert!(matches!(backend.take_calls().as_slice(),
            [Call::ValidateUrl, Call::Register(_, _, mode, _, 500, true, 615)] if *mode == expected_mode));

        set_outcome(&backend, WriteOutcome::Accepted);
        send(&app, register()).await.assert(409, "fee_changed");
        assert!(matches!(backend.take_calls().as_slice(),
            [Call::ValidateUrl, Call::Register(_, _, mode, _, 500, true, 615)] if *mode == expected_mode));
    }
}

#[tokio::test]
async fn mutation_closure_rules_come_from_the_use_case() {
    let backend = Arc::new(Backend::default());
    let app = router(setup(false, &backend));
    Command::Register.send(&app).await.assert(404, "consignment_disabled");
    assert!(matches!(
        backend.take_calls().as_slice(),
        [Call::ValidateUrl, Call::Register(_, _, _, _, _, false, _)]
    ));
    Command::Reprice.send(&app).await.assert(404, "consignment_disabled");
    assert!(matches!(backend.take_calls().as_slice(), [Call::Reprice(_, _, _, _, _, false)]));
    for (error, code) in [
        (Rejection::ListingNotActive, "listing_not_active"),
        (Rejection::StaleVersion, "stale_version"),
    ] {
        set_outcome(&backend, WriteOutcome::Reject(error));
        Command::Withdraw.send(&app).await.assert(409, code);
        assert!(matches!(backend.take_calls().as_slice(), [Call::Withdraw(..)]));
    }
    set_outcome(&backend, WriteOutcome::Reject(Rejection::ListingNotFound));
    for command in [Command::Reprice, Command::Withdraw] {
        let missing = command.send(&app).await;
        let mut other = command.request();
        login(&mut other, OTHER_SELLER);
        let other = send(&app, other).await;
        missing.assert(404, "listing_not_found");
        other.assert(404, "listing_not_found");
        assert_eq!(other.body, missing.body);
        assert!(matches!(backend.take_calls().as_slice(),
            [Call::Reprice(first, ..), Call::Reprice(second, ..)] |
            [Call::Withdraw(first, ..), Call::Withdraw(second, ..)]
            if first.as_uuid() == SELLER && second.as_uuid() == OTHER_SELLER));
    }
}

#[tokio::test]
async fn mutation_maps_each_business_rejection_and_replayed_rejection() {
    for (command, rejection, status, code) in [
        (Command::Register, Rejection::ParticipantIneligible, 403, "participant_ineligible"),
        (Command::Register, Rejection::AlreadyOpen, 409, "already_open"),
        (Command::Register, Rejection::EquipmentBusy, 409, "equipment_busy"),
        (Command::Register, Rejection::HistoryUnproven, 409, "history_unproven"),
        (Command::Register, Rejection::HistoryOwnerMismatch, 409, "history_owner_mismatch"),
        (Command::Register, Rejection::HistoryAliasConflict, 409, "history_alias_conflict"),
        (Command::Register, Rejection::FeeChanged, 409, "fee_changed"),
        (Command::Register, Rejection::WorldUnavailable, 409, "world_unavailable"),
        (Command::Register, Rejection::RegistrationRateLimited, 429, "registration_rate_limited"),
        (Command::Register, Rejection::ListingLimitReached, 429, "listing_limit_reached"),
        (Command::Reprice, Rejection::ListingNotFound, 404, "listing_not_found"),
        (Command::Reprice, Rejection::StaleVersion, 409, "stale_version"),
        (Command::Reprice, Rejection::ListingNotActive, 409, "listing_not_active"),
        (Command::Withdraw, Rejection::ListingNotFound, 404, "listing_not_found"),
        (Command::Withdraw, Rejection::StaleVersion, 409, "stale_version"),
        (Command::Withdraw, Rejection::ListingNotActive, 409, "listing_not_active"),
    ] {
        let backend = Arc::new(Backend::default());
        set_outcome(&backend, WriteOutcome::Reject(rejection));
        let first = command.send(&router(setup(true, &backend))).await;
        first.assert(status, code);
        set_outcome(&backend, WriteOutcome::ReplayRejected(rejection));
        for enabled in [false, true] {
            let replay = command.send(&router(setup(enabled, &backend))).await;
            replay.assert(status, code);
            assert_eq!(replay.body, first.body);
        }
        assert_eq!(
            backend.take_calls().len(),
            if matches!(command, Command::Register) { 6 } else { 3 }
        );
    }
}

#[tokio::test]
async fn mutation_keeps_conflict_unavailable_and_unknown_distinct_for_each_operation() {
    let backend = Arc::new(Backend::default());
    for enabled in [false, true] {
        let app = router(setup(enabled, &backend));
        for (outcome, status, code) in [
            (WriteOutcome::Conflict, 409, "command_id_conflict"),
            (WriteOutcome::Err(ConsignmentError::Unavailable), 503, "consignment_unavailable"),
            (WriteOutcome::Err(ConsignmentError::OutcomeUnknown), 503, "outcome_unknown"),
        ] {
            set_outcome(&backend, outcome);
            for command in COMMANDS {
                command.send(&app).await.assert(status, code);
            }
            assert_eq!(backend.take_calls().len(), 4);
        }
        let mut missing = setup(enabled, &backend);
        missing.capabilities.register = None;
        missing.capabilities.manage = None;
        let app = router(missing);
        for command in COMMANDS {
            command.send(&app).await.assert(503, "consignment_unavailable");
        }
        assert!(backend.take_calls().is_empty());
    }
}

#[tokio::test]
async fn unchanged_and_closed_receipts_preserve_nullable_fields() {
    let backend = Arc::new(Backend::default());
    let app = router(setup(true, &backend));
    set_outcome(&backend, WriteOutcome::Unchanged);
    let mut input = Command::Reprice.input();
    input["command_id"] = Value::from(fixture_uuid(0x67).to_string());
    input["expected_version"] = Value::from(2);
    let mut req = Command::Reprice.request_with(&Command::Reprice.uri(), input.to_string());
    login(&mut req, SELLER);
    let reply = send(&app, req).await;
    reply.assert(200, "ok");
    assert_wire("public/consignment-unchanged-receipt", reply.body);

    set_outcome(&backend, WriteOutcome::CloseWithoutPrice);
    let reply = Command::Withdraw.send(&app).await;
    reply.assert(200, "ok");
    let mut expected = wire_fixture("public/consignment-withdraw-receipt");
    expected["data"]["price"] = Value::Null;
    assert_eq!(reply.body, expected);

    set_outcome(&backend, WriteOutcome::WrongTarget);
    Command::Reprice.send(&app).await.assert(503, "outcome_unknown");
}

#[tokio::test]
async fn withdraw_rejects_a_receipt_for_another_listing() {
    let backend = Arc::new(Backend::default());
    set_outcome(&backend, WriteOutcome::WrongTarget);
    for enabled in [true, false] {
        let reply = Command::Withdraw.send(&router(setup(enabled, &backend))).await;
        reply.assert(503, "outcome_unknown");
        assert!(!reply.body.to_string().contains(&fixture_uuid(99).to_string()));
        assert_eq!(
            backend.take_calls(),
            vec![Call::Withdraw(
                UserId::from_uuid(SELLER),
                CommandId::from_uuid(fixture_uuid(0x68)),
                LISTING,
                2,
            )]
        );
    }
}

#[tokio::test]
async fn mutation_guards_precede_query_path_body_and_all_consignment_ports() {
    for enabled in [false, true] {
        for command in COMMANDS {
            let session = Arc::new(Session::default());
            let backend = Arc::new(Backend::default());
            let app = router(configured(enabled, &backend, Some(session.clone()), 100));
            let uri = match command {
                Command::Register => format!("{LISTINGS}?bad=1"),
                Command::Reprice => format!("{LISTINGS}/bad-id/price?bad=1"),
                Command::Withdraw => format!("{LISTINGS}/bad-id/withdraw?bad=1"),
            };
            let invalid = || {
                let mut req = command.request_with(&uri, FAKE_LINK.to_owned());
                req.headers_mut()
                    .insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
                req
            };

            let mut wrong_origin = invalid();
            wrong_origin
                .headers_mut()
                .insert(header::ORIGIN, HeaderValue::from_static("https://other.invalid"));
            send(&app, wrong_origin).await.assert(403, "origin_forbidden");
            assert_eq!(session.take_calls(), 0);

            send(&app, invalid()).await.assert(401, "unauthenticated");
            assert_eq!(session.take_calls(), 0);
            let mut expired = invalid();
            expired.headers_mut().insert(header::COOKIE, HeaderValue::from_static(EXPIRED_COOKIE));
            send(&app, expired).await.assert(401, "unauthenticated");
            assert_eq!(session.take_calls(), 1);

            for (expected, status, code) in [
                ("bad-id".to_owned(), 400, "invalid_expected_user_id"),
                (OTHER_SELLER.to_string(), 409, "session_changed"),
            ] {
                let mut req = invalid();
                login(&mut req, SELLER);
                req.headers_mut().insert(
                    PUBLIC_EXPECTED_USER_ID_HEADER,
                    HeaderValue::from_str(&expected).expect("预期用户头"),
                );
                send(&app, req).await.assert(status, code);
                assert_eq!(session.take_calls(), 1);
            }
            let mut guarded = invalid();
            login(&mut guarded, SELLER);
            send(&app, guarded).await.assert(400, "invalid_query");
            assert_eq!(session.take_calls(), 1);

            let limited = router(configured(enabled, &backend, Some(session.clone()), 1));
            let mut first = invalid();
            first
                .headers_mut()
                .insert(header::ORIGIN, HeaderValue::from_static("https://other.invalid"));
            send(&limited, first).await.assert(403, "origin_forbidden");
            let mut next = invalid();
            next.headers_mut()
                .insert(header::ORIGIN, HeaderValue::from_static("https://other.invalid"));
            let reply = send(&limited, next).await;
            reply.assert(429, "rate_limited");
            assert!(reply.headers.contains_key(header::RETRY_AFTER));
            assert_eq!(session.take_calls(), 0);
            assert!(backend.take_calls().is_empty());
        }
    }
}

#[tokio::test]
async fn mutation_rejects_invalid_body_and_path_before_use_cases() {
    let backend = Arc::new(Backend::default());
    for enabled in [false, true] {
        let app = router(setup(enabled, &backend));
        for command in COMMANDS {
            let valid = command.input().to_string();
            let invalids = [
                (format!("{valid} true"), "invalid_listing_request"),
                (format!(r#"{{"unknown":"{FAKE_LINK}"}}"#), "invalid_listing_request"),
                (format!(r#"{{"actor":"{}",&}}"#, OTHER_SELLER), "invalid_listing_request"),
                ("null".to_owned(), "invalid_listing_request"),
                ("[]".to_owned(), "invalid_listing_request"),
                ("{".to_owned(), "invalid_listing_request"),
                (
                    format!(
                        r#"{{"command_id":"{}",{}"#,
                        command.input()["command_id"].as_str().expect("UUID"),
                        &valid[1..]
                    ),
                    "invalid_listing_request",
                ),
                (
                    valid.replace(
                        command.input()["command_id"].as_str().expect("UUID"),
                        "00000000-0000-0000-0000-000000000000",
                    ),
                    "invalid_command_id",
                ),
                (
                    valid.replace(command.input()["command_id"].as_str().expect("UUID"), FAKE_LINK),
                    "invalid_command_id",
                ),
            ];
            for (body, code) in invalids {
                let mut req = command.request_with(&command.uri(), body);
                login(&mut req, SELLER);
                send(&app, req).await.assert(400, code);
            }
            let mut actor = command.input();
            actor["actor"] = Value::from(OTHER_SELLER.to_string());
            let mut req = command.request_with(&command.uri(), actor.to_string());
            login(&mut req, SELLER);
            send(&app, req).await.assert(400, "invalid_listing_request");

            if matches!(command, Command::Register | Command::Reprice) {
                for raw in [
                    "0",
                    "-1",
                    "1000000001",
                    "1.0",
                    "1e2",
                    "1e400",
                    r#""102""#,
                    "null",
                    "true",
                    "9223372036854775808",
                ] {
                    let number = command.input()["price"].to_string();
                    let body = valid
                        .replace(&format!(r#""price":{number}"#), &format!(r#""price":{raw}"#));
                    let mut req = command.request_with(&command.uri(), body);
                    login(&mut req, SELLER);
                    send(&app, req).await.assert(400, "invalid_price");
                }
            }
            if matches!(command, Command::Register) {
                for (field, raw, code) in [
                    ("mode", serde_json::json!("other"), "invalid_listing_request"),
                    ("mode", serde_json::json!({"consigned": null}), "invalid_listing_request"),
                    ("expected_fee_bps", serde_json::json!("500"), "invalid_listing_request"),
                    ("expected_fee_bps", serde_json::json!(-1), "invalid_listing_request"),
                    ("expected_fee_bps", serde_json::json!(10000), "invalid_listing_request"),
                    ("equipment_url", serde_json::json!(42), "invalid_equipment_url"),
                ] {
                    let mut invalid = command.input();
                    invalid[field] = raw;
                    let mut req = command.request_with(&command.uri(), invalid.to_string());
                    login(&mut req, SELLER);
                    send(&app, req).await.assert(400, code);
                }
            } else {
                for raw in ["0", "-1", "9007199254740992", "1.0", r#""1""#, "null"] {
                    let number = command.input()["expected_version"].to_string();
                    let body = valid.replace(
                        &format!(r#""expected_version":{number}"#),
                        &format!(r#""expected_version":{raw}"#),
                    );
                    let mut req = command.request_with(&command.uri(), body);
                    login(&mut req, SELLER);
                    send(&app, req).await.assert(400, "invalid_listing_request");
                }
                for invalid_id in ["invalid", "%FF", "00000000-0000-0000-0000-000000000000"] {
                    let uri = command.uri().replace(&LISTING.to_string(), invalid_id);
                    let mut req = command.request_with(&uri, "bad body".to_owned());
                    login(&mut req, SELLER);
                    send(&app, req).await.assert(400, "invalid_listing_id");
                }
            }
            assert!(backend.take_calls().is_empty());
        }

        let mut invalid = Command::Register.input();
        invalid["equipment_url"] = Value::from(format!("{FAKE_LINK}/invalid"));
        let mut req = Command::Register.request_with(LISTINGS, invalid.to_string());
        login(&mut req, SELLER);
        send(&app, req).await.assert(400, "invalid_equipment_url");
        assert_eq!(backend.take_calls(), vec![Call::ValidateUrl]);
    }
}

#[tokio::test]
async fn malformed_json_is_rejected_before_field_classification() {
    let backend = Arc::new(Backend::default());
    let app = router(setup(true, &backend));
    let mut incorrect_codes = Vec::new();
    for (case, raw) in [
        ("截断命令 ID", br#"{"command_id":"0000"#.as_slice()),
        ("截断装备链接", br#"{"equipment_url":"https://fixture.invalid"#),
        ("标价缺冒号", br#"{"price" 15}"#),
        ("标价指数截断", br#"{"price":15e}"#),
        ("命令 ID 的转义无效", br#"{"command_id":"\x"}"#),
        ("字段值错误后存在坏语法", br#"{"price":null,"mode":}"#),
    ] {
        let mut req = Command::Register.request();
        *req.body_mut() = Body::from(raw);
        let reply = send(&app, req).await;
        assert_eq!(reply.status, 400, "{case}");
        assert!(reply.body["data"].is_null());
        if reply.body["code"] != "invalid_listing_request" {
            incorrect_codes.push((case, reply.body["code"].clone()));
        }
        assert!(backend.take_calls().is_empty());
    }
    assert!(incorrect_codes.is_empty(), "语法错误分类不符：{incorrect_codes:?}");
}

#[tokio::test]
async fn invalid_unicode_escapes_are_syntax_errors_for_every_mutation() {
    let backend = Arc::new(Backend::default());
    let mut incorrect_codes = Vec::new();
    for enabled in [false, true] {
        let app = router(setup(enabled, &backend));
        for (command, key_body, value_body) in [
            (
                Command::Register,
                br#"{"equipment_url":null,"\uD800":0}"#.as_slice(),
                br#"{"equipment_url":"\uD800"}"#.as_slice(),
            ),
            (
                Command::Reprice,
                br#"{"price":null,"\uD800":0}"#.as_slice(),
                br#"{"price":"\uDC00"}"#.as_slice(),
            ),
            (
                Command::Withdraw,
                br#"{"command_id":null,"\uDC00":0}"#.as_slice(),
                br#"{"command_id":"\uDC00"}"#.as_slice(),
            ),
        ] {
            for (position, raw) in [("键", key_body), ("值", value_body)] {
                let mut req = command.request();
                *req.body_mut() = Body::from(raw);
                let reply = send(&app, req).await;
                assert_eq!(reply.status, 400, "{command:?} {position} enabled={enabled}");
                assert!(reply.body["data"].is_null());
                if reply.body["code"] != "invalid_listing_request" {
                    incorrect_codes.push((command, position, enabled, reply.body["code"].clone()));
                }
                assert!(backend.take_calls().is_empty());
            }
        }
    }
    assert!(incorrect_codes.is_empty(), "Unicode 转义分类不符：{incorrect_codes:?}");
}

#[tokio::test]
async fn valid_unicode_escapes_preserve_overflowing_price_classification() {
    let backend = Arc::new(Backend::default());
    let app = router(setup(true, &backend));
    let mut req = Command::Reprice.request();
    *req.body_mut() = Body::from(
        br#"{"price":1e400,"\uD83D\uDE00":"\\uD800\"","\u0061":"\uD83D\uDE00"}"#.as_slice(),
    );
    send(&app, req).await.assert(400, "invalid_price");
    assert!(backend.take_calls().is_empty());
}

#[tokio::test]
async fn valid_price_and_fee_endpoints_reach_the_use_cases_unchanged() {
    for (price, fee_bps) in [(1, 0_u16), (1_000_000_000, 9999)] {
        let backend = Arc::new(Backend::default());
        let mut state = setup(true, &backend);
        let fee = FeeBps::new(i64::from(fee_bps)).expect("配置费率");
        state.capabilities.fee_bps = fee;
        state.capabilities.register = Some(register_service(&backend, true, fee));
        let app = router(state);

        let mut input = Command::Register.input();
        input["price"] = Value::from(price);
        input["expected_fee_bps"] = Value::from(fee_bps);
        let mut req = Command::Register.request_with(LISTINGS, input.to_string());
        login(&mut req, SELLER);
        let reply = send(&app, req).await;
        reply.assert(202, "accepted");
        assert_eq!(reply.body["data"]["price"], price);
        assert_eq!(
            backend.take_calls(),
            vec![
                Call::ValidateUrl,
                Call::Register(
                    UserId::from_uuid(SELLER),
                    CommandId::from_uuid(fixture_uuid(0x65)),
                    ListingMode::Consigned,
                    price,
                    fee_bps,
                    true,
                    fee_bps,
                ),
            ]
        );

        let mut input = Command::Reprice.input();
        input["price"] = Value::from(price);
        let mut req = Command::Reprice.request_with(&Command::Reprice.uri(), input.to_string());
        login(&mut req, SELLER);
        let reply = send(&app, req).await;
        reply.assert(200, "ok");
        assert_eq!(reply.body["data"]["price"], price);
        assert_eq!(
            backend.take_calls(),
            vec![Call::Reprice(
                UserId::from_uuid(SELLER),
                CommandId::from_uuid(fixture_uuid(0x66)),
                LISTING,
                1,
                price,
                true,
            )]
        );
    }
}

#[tokio::test]
async fn mutation_body_size_and_media_errors_use_existing_wire_codes() {
    let backend = Arc::new(Backend::default());
    for enabled in [false, true] {
        let app = router(setup(enabled, &backend));
        for command in COMMANDS {
            let mut bounded = command.input().to_string();
            bounded.push_str(&" ".repeat(4096 - bounded.len()));
            let mut req = command.request_with(&command.uri(), bounded.clone() + " ");
            login(&mut req, SELLER);
            send(&app, req).await.assert(413, "body_too_large");
            for media in [None, Some("text/plain"), Some("application/problem+json")] {
                let mut req = command.request();
                req.headers_mut().remove(header::CONTENT_TYPE);
                if let Some(media) = media {
                    req.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(media));
                }
                send(&app, req).await.assert(415, "unsupported_media_type");
            }
            assert!(backend.take_calls().is_empty());

            let mut req = command.request_with(&command.uri(), bounded);
            req.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json; charset=utf-8"),
            );
            login(&mut req, SELLER);
            if !enabled && matches!(command, Command::Withdraw) {
                set_outcome(&backend, WriteOutcome::Reject(Rejection::ListingNotActive));
            }
            let reply = send(&app, req).await;
            match (enabled, command) {
                (true, Command::Register) => reply.assert(202, "accepted"),
                (false, Command::Register | Command::Reprice) => {
                    reply.assert(404, "consignment_disabled");
                }
                (false, Command::Withdraw) => reply.assert(409, "listing_not_active"),
                _ => reply.assert(200, "ok"),
            }
            assert!(!backend.take_calls().is_empty());
            set_outcome(&backend, WriteOutcome::Accepted);
        }
    }
}
