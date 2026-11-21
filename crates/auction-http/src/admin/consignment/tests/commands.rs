//! Admin 下架 HTTP 解码、命令交接、关闭与回执测试。

use std::sync::Arc;

use auction_application::consignment::{
    ConsignmentError, ListingBusinessRejection, ListingClosed, ListingCommandOutcome,
    ListingCommandRejection, ManageListingService, StoredListingDecision,
    UnrecordedListingRejection,
};
use auction_domain::consignment::{ListingPrice, ListingVersion};
use axum::body::Body;
use axum::http::{HeaderValue, Method, StatusCode, header};
use serde_json::{Value, json};
use uuid::Uuid;

use super::fixture::{
    ACTOR, COMMAND, Fixture, LIST_PATH, LISTING, REASON, SessionCall, body, delist, delist_path,
    failure, headers, response, send,
};

#[tokio::test]
async fn delist_success_and_closed_replay_return_identical_receipt_with_session_actor() {
    let mut fixture = Fixture::new(true);
    let original =
        ListingClosed::new(LISTING, ListingVersion::new(8).unwrap(), ListingPrice::new(101).ok())
            .unwrap();
    fixture.command.results.lock().unwrap().extend([
        Ok(ListingCommandOutcome::Accepted(original)),
        Ok(ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(original))),
        Ok(ListingCommandOutcome::Rejected(ListingCommandRejection::Recorded(
            ListingBusinessRejection::ListingNotActive,
        ))),
    ]);
    let accepted = response(delist(&fixture, body(REASON)).await, StatusCode::OK).await;
    assert_eq!(accepted["code"], "ok");
    assert_eq!(
        accepted["data"],
        json!({
            "command_id": COMMAND, "operation": "admin_delist", "listing_id": LISTING,
            "listing_version": 8, "state":"closed", "disposition":"closed", "price":101
        })
    );
    assert!(!accepted.to_string().contains(REASON));

    assert!(fixture.read_port().calls.lock().unwrap().is_empty());
    // 保留同一账本端口，切换为停用投影与关闭配置。HTTP 仍应传递原命令。
    fixture.read = Fixture::new(false).read;
    fixture.manage = Some(Arc::new(ManageListingService::new(fixture.command.clone(), false)));
    let replay = response(delist(&fixture, body(REASON)).await, StatusCode::OK).await;
    assert_eq!(accepted, replay);

    let mut fresh = body(REASON);
    let new_command = Uuid::from_u128(0xbabe);
    fresh["command_id"] = json!(new_command);
    failure(delist(&fixture, fresh).await, StatusCode::CONFLICT, "listing_not_active").await;

    let commands = fixture.command.calls.lock().unwrap();
    assert_eq!(commands.len(), 3);
    assert_eq!(commands[0], commands[1]);
    assert_eq!(commands[0].command_id.as_uuid(), COMMAND);
    assert_eq!(commands[0].listing_id, LISTING);
    assert_eq!(commands[0].expected_version.value(), 7);
    assert_eq!(commands[0].admin_actor, ACTOR);
    assert!(commands[0].reason.as_str() == REASON);
    assert_eq!(commands[2].command_id.as_uuid(), new_command);
    assert!(fixture.read_port().calls.lock().unwrap().is_empty());
    assert_eq!(
        fixture.sessions.0.lock().unwrap().as_slice(),
        &[SessionCall::Touch, SessionCall::Touch, SessionCall::Touch],
    );
}

#[tokio::test]
async fn delist_rejections_and_unknown_outcomes_keep_distinct_codes_in_both_configs() {
    for enabled in [true, false] {
        let fixture = Fixture::new(enabled);
        for (outcome, status, code) in [
            (
                rejected(ListingBusinessRejection::ListingNotFound),
                StatusCode::NOT_FOUND,
                "listing_not_found",
            ),
            (
                rejected(ListingBusinessRejection::StaleVersion),
                StatusCode::CONFLICT,
                "stale_version",
            ),
            (
                rejected(ListingBusinessRejection::ListingNotActive),
                StatusCode::CONFLICT,
                "listing_not_active",
            ),
            (
                Ok(ListingCommandOutcome::Rejected(ListingCommandRejection::Unrecorded(
                    UnrecordedListingRejection::CommandIdConflict,
                ))),
                StatusCode::CONFLICT,
                "command_id_conflict",
            ),
            (
                Err(ConsignmentError::Unavailable),
                StatusCode::SERVICE_UNAVAILABLE,
                "consignment_unavailable",
            ),
            (
                Err(ConsignmentError::OutcomeUnknown),
                StatusCode::SERVICE_UNAVAILABLE,
                "outcome_unknown",
            ),
            (
                Err(ConsignmentError::Disabled),
                StatusCode::SERVICE_UNAVAILABLE,
                "consignment_unavailable",
            ),
        ] {
            fixture.command.results.lock().unwrap().push_back(outcome);
            let reply = failure(delist(&fixture, body(REASON)).await, status, code).await;
            assert!(!reply.to_string().contains(REASON));
            if code == "outcome_unknown" {
                assert!(reply["message"].as_str().unwrap().contains("原样重试"));
            }
        }
        assert_eq!(fixture.command.calls.lock().unwrap().len(), 7);
        assert!(fixture.read_port().calls.lock().unwrap().is_empty());
    }
}

fn rejected(
    error: ListingBusinessRejection,
) -> Result<ListingCommandOutcome<ListingClosed>, ConsignmentError> {
    Ok(ListingCommandOutcome::Rejected(ListingCommandRejection::Recorded(error)))
}

#[tokio::test]
async fn delist_replayed_rejection_keeps_original_status_and_body() {
    let fixture = Fixture::new(false);
    let stale = ListingBusinessRejection::StaleVersion;
    fixture.command.results.lock().unwrap().extend([
        rejected(stale),
        Ok(ListingCommandOutcome::Replayed(StoredListingDecision::Rejected(stale))),
    ]);
    let original =
        failure(delist(&fixture, body(REASON)).await, StatusCode::CONFLICT, "stale_version").await;
    let replay =
        failure(delist(&fixture, body(REASON)).await, StatusCode::CONFLICT, "stale_version").await;
    assert_eq!(original, replay);
    let commands = fixture.command.calls.lock().unwrap();
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0], commands[1]);
}

#[tokio::test]
async fn delist_without_port_returns_503_but_bad_input_precedes_dependency_lookup() {
    let mut fixture = Fixture::new(false);
    fixture.manage = None;
    failure(
        delist(&fixture, body(REASON)).await,
        StatusCode::SERVICE_UNAVAILABLE,
        "consignment_unavailable",
    )
    .await;
    failure(delist(&fixture, body("")).await, StatusCode::BAD_REQUEST, "invalid_reason").await;
    fixture.assert_no_business_calls();
}

#[tokio::test]
async fn delist_rejects_every_invalid_reason_without_echoing_or_calling_ports() {
    for reason in [
        String::new(),
        "字".repeat(86),
        " 说明夹具".to_owned(),
        "说明夹具 ".to_owned(),
        "说明夹具\n另起一行".to_owned(),
        "说明夹具\r另起一行".to_owned(),
        "说明夹具\u{0085}另起一行".to_owned(),
        "说明夹具\u{2028}另起一行".to_owned(),
        "说明夹具\u{2029}另起一行".to_owned(),
        "说明夹具 https://equipment.example.invalid/equip/42/0000000000".to_owned(),
        "说明夹具 https://hvdata.example.invalid/equip?eid=42&key=0000000000".to_owned(),
        "说明夹具 https://equipment.example.invalid/?key=0000000000".to_owned(),
        "说明夹具 0000000000".to_owned(),
        format!("说明夹具 {}", "z".repeat(43)),
    ] {
        let mut fixture = Fixture::new(true);
        for connected in [true, false] {
            if !connected {
                fixture.manage = None;
            }
            let reply = failure(
                delist(&fixture, body(&reason)).await,
                StatusCode::BAD_REQUEST,
                "invalid_reason",
            )
            .await;
            assert!(!reply.to_string().contains("说明夹具"));
            assert!(!reply.to_string().contains("0000000000"));
        }
        fixture.assert_no_business_calls();
    }
}

#[tokio::test]
async fn delist_rejects_noncanonical_targets_and_strict_json_shape_before_port() {
    let fixture = Fixture::new(true);
    let content = body(REASON);
    let valid = content.to_string();
    for path in [
        format!("{LIST_PATH}/bad-id/delist"),
        format!("{LIST_PATH}/%ff/delist"),
        format!("{LIST_PATH}/{}/delist", Uuid::nil()),
        format!("{LIST_PATH}/{}/delist", LISTING.to_string().to_uppercase()),
    ] {
        let reply = send(fixture.router(), Method::POST, &path, headers(), Body::empty()).await;
        failure(reply, StatusCode::BAD_REQUEST, "invalid_listing_id").await;
    }
    let with_query = format!("{}?extra=1", delist_path());
    let reply =
        send(fixture.router(), Method::POST, &with_query, headers(), Body::from(valid.clone()))
            .await;
    failure(reply, StatusCode::BAD_REQUEST, "invalid_query").await;

    for command_id in [
        json!("bad-id"),
        json!(Uuid::nil()),
        json!(COMMAND.to_string().to_uppercase()),
        json!(COMMAND.simple().to_string()),
        json!(1),
    ] {
        let mut input = content.clone();
        input["command_id"] = command_id;
        failure(delist(&fixture, input).await, StatusCode::BAD_REQUEST, "invalid_command_id").await;
    }

    for version in
        [json!(0), json!(-1), json!(1.0), json!("7"), json!(null), json!(9_007_199_254_740_992_u64)]
    {
        let mut input = content.clone();
        input["expected_version"] = version;
        invalid_body(&fixture, input.to_string()).await;
    }
    for field in ["command_id", "expected_version", "reason"] {
        let mut missing = content.clone();
        let duplicate = format!(r#"{{"{field}":{},{}"#, content[field], &valid[1..]);
        missing.as_object_mut().unwrap().remove(field);
        invalid_body(&fixture, missing.to_string()).await;
        invalid_body(&fixture, duplicate).await;
    }
    let mut caller_actor = content.clone();
    caller_actor["admin_actor"] = json!("different-admin");
    for bad in [
        "{}".to_owned(),
        "{".to_owned(),
        "null".to_owned(),
        json!([COMMAND, 7, REASON]).to_string(),
        caller_actor.to_string(),
        format!("{valid} null"),
        valid.replace(":7", ":7e0"),
        valid.replace(":7", ":18446744073709551616"),
    ] {
        invalid_body(&fixture, bad).await;
    }
    fixture.assert_no_business_calls();
}

async fn invalid_body(fixture: &Fixture, content: String) {
    let reply =
        send(fixture.router(), Method::POST, &delist_path(), headers(), Body::from(content)).await;
    failure(reply, StatusCode::BAD_REQUEST, "invalid_listing_request").await;
}

#[tokio::test]
async fn delist_requires_json_and_caps_body_at_4096_bytes() {
    let fixture = Fixture::new(true);
    let path = delist_path();
    let valid = body(REASON).to_string();
    for media in [None, Some("text/plain"), Some("application/problem+json")] {
        let mut request_headers = headers();
        if let Some(media) = media {
            request_headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(media));
        } else {
            request_headers.remove(header::CONTENT_TYPE);
        }
        let reply =
            send(fixture.router(), Method::POST, &path, request_headers, Body::from(valid.clone()))
                .await;
        failure(reply, StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_media_type").await;
    }
    let base = body(&"字".repeat(85)).to_string();
    let padding = " ".repeat(4096 - base.len());
    let exact = format!("{base}{padding}");
    let reply =
        send(fixture.router(), Method::POST, &path, headers(), Body::from(format!("{exact} ")))
            .await;
    failure(reply, StatusCode::PAYLOAD_TOO_LARGE, "body_too_large").await;
    fixture.assert_no_business_calls();
    fixture.command.results.lock().unwrap().push_back(Ok(ListingCommandOutcome::Accepted(
        ListingClosed::new(LISTING, ListingVersion::new(8).unwrap(), None).unwrap(),
    )));
    let reply = send(fixture.router(), Method::POST, &path, headers(), Body::from(exact)).await;
    let reply = response(reply, StatusCode::OK).await;
    assert!(reply["data"].get("price").is_some_and(Value::is_null));
}

#[tokio::test]
async fn delist_wrong_receipt_target_or_version_is_unknown() {
    let fixture = Fixture::new(true);
    for (listing_id, version) in [(Uuid::from_u128(42), 8), (LISTING, 7), (LISTING, 9)] {
        fixture.command.results.lock().unwrap().push_back(Ok(ListingCommandOutcome::Accepted(
            ListingClosed::new(listing_id, ListingVersion::new(version).unwrap(), None).unwrap(),
        )));
        failure(
            delist(&fixture, body(REASON)).await,
            StatusCode::SERVICE_UNAVAILABLE,
            "outcome_unknown",
        )
        .await;
    }
}

#[tokio::test]
async fn delist_timeout_reports_unknown_and_does_not_resubmit() {
    let mut fixture = Fixture::new(false);
    let port = Arc::new(super::fixture::CommandPort { pending: true, ..Default::default() });
    fixture.manage = Some(Arc::new(ManageListingService::new(port.clone(), false)));
    failure(
        delist(&fixture, body(REASON)).await,
        StatusCode::SERVICE_UNAVAILABLE,
        "outcome_unknown",
    )
    .await;
    assert_eq!(port.calls.lock().unwrap().len(), 1);
}
