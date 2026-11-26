//! 免登录非寄售登记与回执查询的来源守卫、每来源限流和身份边界。

use std::sync::Arc;

use auction_application::consignment::ConsignmentError;
use auction_application::consignment::{
    AnonymousRegistrationReceipt, ListingLookupProgress, ListingRegistered,
    OwnedListingCommandReceipt, OwnedListingDecision, PrivateListing, PrivateListingView,
    PrivateListingViewKind, StoredListingDecision,
};
use auction_domain::CommandId;
use auction_domain::consignment::{FeeBps, ListingEquipmentIdentity, ListingMode, ListingState};
use axum::body::Body;
use axum::http::{HeaderValue, Method, Request, header};
use serde_json::{Value, json};

use crate::public::consignment::tests::fixtures::{
    Backend, Call, DIRECT_COMMANDS, DIRECT_LISTINGS, FAKE_LINK, GOOD_ORIGIN, LISTING, SELLER,
    fixture_uuid, get, login, price, request, router, send, setup, time, version,
};

const NAME: &str = "Anon Seller";

fn direct_request(command: u128, origin: &'static str) -> Request<Body> {
    let body = json!({
        "command_id": fixture_uuid(command),
        "seller_game_name": NAME,
        "equipment_url": FAKE_LINK,
        "price": 101,
    });
    let mut req = request(Method::POST, DIRECT_LISTINGS, Body::from(body.to_string()));
    req.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    req.headers_mut().insert(header::ORIGIN, HeaderValue::from_static(origin));
    req
}

fn accepted_receipt(command_id: CommandId) -> AnonymousRegistrationReceipt {
    let created_at = time("2026-11-20T18:00:00Z");
    let expires_at = time("2026-11-21T18:00:00Z");
    let progress =
        ListingLookupProgress::new(0, Some(created_at), None, false, expires_at).expect("解析进度");
    AnonymousRegistrationReceipt {
        command: OwnedListingCommandReceipt {
            command_id,
            decision: OwnedListingDecision::Register(StoredListingDecision::Accepted(
                ListingRegistered::new(LISTING, price(101)).expect("登记回执"),
            )),
        },
        listing: Some(PrivateListing {
            listing_id: LISTING,
            version: version(1),
            identity: ListingEquipmentIdentity::new("main", "42").expect("装备身份"),
            price: price(101),
            pricing_version: 1,
            fee_bps: FeeBps::DIRECT,
            created_at,
            updated_at: created_at,
            expires_at,
            view: PrivateListingView::new(
                ListingMode::Direct,
                ListingState::Resolving,
                PrivateListingViewKind::Resolving(progress),
            )
            .expect("挂单视图"),
        }),
    }
}

#[tokio::test]
async fn direct_registration_needs_no_session_and_receipt_reads_by_command_id() {
    let command_id = CommandId::from_uuid(fixture_uuid(0x71));
    let backend =
        Arc::new(Backend { anonymous: Some(accepted_receipt(command_id)), ..Backend::default() });
    let app = router(setup(true, &backend));

    // 不变量 1、6、7：无 Cookie 也受理；模式固定非寄售、费率为零，卖家只取自填名。
    let reply = send(&app, direct_request(0x71, GOOD_ORIGIN)).await;
    reply.assert(202, "accepted");
    assert_eq!(reply.body["data"]["listing_id"], LISTING.to_string());
    assert_eq!(reply.body["data"]["state"], "resolving");
    assert_eq!(
        backend.take_calls(),
        vec![
            Call::AnonymousCommand(command_id),
            Call::ValidateUrl,
            Call::RegisterAnonymous(NAME.to_owned(), command_id, ListingMode::Direct, 101, 0),
        ]
    );

    // 带会话的请求也不读会话：同一入口始终按自填名登记。
    let mut with_session = direct_request(0x72, GOOD_ORIGIN);
    login(&mut with_session, SELLER);
    send(&app, with_session).await.assert(202, "accepted");
    assert!(matches!(
        backend.take_calls().as_slice(),
        [Call::AnonymousCommand(_), Call::ValidateUrl, Call::RegisterAnonymous(..)]
    ));

    let receipt = send(&app, get(&format!("{DIRECT_COMMANDS}/{}", fixture_uuid(0x71)))).await;
    receipt.assert(200, "ok");
    let data = &receipt.body["data"];
    assert_eq!(data["result"]["http_status"], 202);
    assert_eq!(data["result"]["code"], "accepted");
    assert_eq!(data["listing"]["effective_state"], "resolving");
    assert_eq!(data["listing"]["effective_reason"], Value::Null);
    assert!(!receipt.body.to_string().contains(NAME), "回执不得回显自填名");
    assert_eq!(backend.take_calls(), vec![Call::AnonymousCommand(command_id)]);
}

#[tokio::test]
async fn unchanged_retry_of_a_recorded_command_is_not_limited_by_the_source_quota() {
    // 不变量 1：每来源额度只限制新命令；已入账命令的原样重试在额度用尽后仍交给用例仲裁。
    let recorded = CommandId::from_uuid(fixture_uuid(0xa1));
    let backend =
        Arc::new(Backend { anonymous: Some(accepted_receipt(recorded)), ..Backend::default() });
    let app = router(setup(true, &backend));
    for command in 0xa2..=0xa4 {
        send(&app, direct_request(command, GOOD_ORIGIN)).await.assert(202, "accepted");
    }
    send(&app, direct_request(0xa5, GOOD_ORIGIN)).await.assert(429, "rate_limited");
    backend.take_calls();

    let replay = send(&app, direct_request(0xa1, GOOD_ORIGIN)).await;
    replay.assert(202, "accepted");
    assert!(matches!(
        backend.take_calls().as_slice(),
        [Call::AnonymousCommand(id), Call::ValidateUrl, Call::RegisterAnonymous(..)] if *id == recorded
    ));
}

#[tokio::test]
async fn ledger_read_failure_fails_closed_without_consuming_the_source_quota() {
    // 不变量 1、6：账本读取失败时无法区分重放与新命令，返回不可用且不扣额度、不进入用例。
    let healthy = Arc::new(Backend::default());
    let failing =
        Arc::new(Backend { read_error: Some(ConsignmentError::Unavailable), ..Backend::default() });
    let state = setup(true, &healthy);
    let mut broken = state.clone();
    broken.capabilities.public_read = Some(failing.clone());
    // 两个路由共用同一份安全策略，即同一来源额度。
    let broken_app = router(broken);
    let app = router(state);
    for command in 0xb1..=0xb3 {
        send(&broken_app, direct_request(command, GOOD_ORIGIN))
            .await
            .assert(503, "consignment_unavailable");
        let command_id = CommandId::from_uuid(fixture_uuid(command));
        assert_eq!(failing.take_calls(), vec![Call::AnonymousCommand(command_id)]);
    }
    assert!(healthy.take_calls().is_empty(), "读取失败不得进入用例");

    // 默认额度 3 仍完整可用，第 4 个新命令才被限流。
    for command in 0xb4..=0xb6 {
        send(&app, direct_request(command, GOOD_ORIGIN)).await.assert(202, "accepted");
    }
    send(&app, direct_request(0xb7, GOOD_ORIGIN)).await.assert(429, "rate_limited");
}

#[tokio::test]
async fn direct_registration_rejects_foreign_origin_and_limits_each_source() {
    let backend = Arc::new(Backend::default());
    let app = router(setup(true, &backend));

    // Origin 不可信时在解码与用例之前拒绝，且不消耗免登录登记额度。
    send(&app, direct_request(0x80, "https://evil.invalid")).await.assert(403, "origin_forbidden");
    assert!(backend.take_calls().is_empty());

    // 默认每来源每小时 3 次；第 4 次在用例之前返回 429 并给出 Retry-After。
    for command in 0x81..=0x83 {
        send(&app, direct_request(command, GOOD_ORIGIN)).await.assert(202, "accepted");
    }
    backend.take_calls();
    let limited = send(&app, direct_request(0x84, GOOD_ORIGIN)).await;
    limited.assert(429, "rate_limited");
    assert!(limited.headers.contains_key(header::RETRY_AFTER));
    // 只查过命令是否已入账，额度用尽的新命令不进入用例。
    let new_command = CommandId::from_uuid(fixture_uuid(0x84));
    assert_eq!(backend.take_calls(), vec![Call::AnonymousCommand(new_command)]);

    // 未知命令 ID 与其他用户的命令一样只得到 404，不区分是否存在。
    let missing = send(&app, get(&format!("{DIRECT_COMMANDS}/{}", fixture_uuid(0x90)))).await;
    missing.assert(404, "command_not_found");
}
