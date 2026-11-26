//! 寄售状态、公开目录与本人历史的路由级契约和校验顺序。

use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{
    ConsignmentError, ListingBusinessRejection, ListingClosed, ListingEquipmentDetails,
    ListingEquipmentLabel, ListingEquipmentLevel, ListingEquipmentStat, ListingEquipmentStatGroup,
    ListingLookupProgress, ListingPageWindow, ListingPriceDisposition, ListingPriceReceipt,
    ListingRegistered, ListingRegistrationAvailability, ListingWorld, ListingWorlds,
    LookupTransientReason, OwnedListingCommandReceipt, OwnedListingDecision, OwnedListingRecord,
    PrivateListing, PrivateListingView, PrivateListingViewKind, PublicListingDetail,
    PublicListingState, PublicListingSummary, SellerListingAction, StoredListingDecision,
};
use auction_contracts::PUBLIC_EXPECTED_USER_ID_HEADER;
use auction_domain::consignment::{
    FeeBps, ListingEquipmentIdentity, ListingMode, ListingReason, ListingState,
};
use auction_domain::{CommandId, UserId};
use axum::body::Body;
use axum::http::{HeaderValue, Method, header};
use serde_json::Value;
use tower::ServiceExt;

use crate::middleware::boundary::HttpBoundary;
use crate::public::consignment::tests::fixtures::{
    Backend, COMMANDS, Call, EXPIRED_COOKIE, FAKE_KEY, FAKE_LINK, LISTING, LISTINGS, OTHER_SELLER,
    OWNED, SELLER, STATUS, Session, configured, fixture_uuid, get, login, price, request, router,
    router_with_boundary, send, setup, time, version,
};
use crate::public::consignment::{ConsignmentCapabilities, mount};
use crate::server::contract_fixtures::{assert_wire, wire_fixture};

fn detail(consigned: bool) -> PublicListingDetail {
    let (listing_id, world, eid, name, category, level, alt_name, state, expires) = if consigned {
        (
            fixture_uuid(5),
            "isekai",
            "43",
            "Fixture Oak Staff",
            "Staff",
            ListingEquipmentLevel::Unassigned,
            Some("合成测试橡木杖".to_owned()),
            PublicListingState::AwaitingDeposit,
            "2026-11-27T18:10:00Z",
        )
    } else {
        (
            fixture_uuid(4),
            "main",
            "42424242424242424242424242424242",
            "Fixture Cotton Robe",
            "Cloth",
            ListingEquipmentLevel::Assigned(42),
            None,
            PublicListingState::Displayed,
            "2026-12-20T18:10:00Z",
        )
    };
    let observed_at = time("2026-11-20T18:10:00Z");
    PublicListingDetail {
        summary: PublicListingSummary {
            listing_id,
            version: version(2),
            state,
            identity: ListingEquipmentIdentity::new(world, eid).expect("装备身份"),
            name: name.to_owned(),
            category: category.to_owned(),
            level,
            price: price(101),
            seller_name: "FixtureSeller".to_owned(),
            seller_verified: true,
            updated_at: observed_at,
            expires_at: time(expires),
        },
        equipment: ListingEquipmentDetails::new(
            observed_at,
            ListingEquipmentLabel {
                name: name.to_owned(),
                alt_name,
                category: category.to_owned(),
                level,
                tradeable: true,
            },
            vec![ListingEquipmentStatGroup {
                name: "Fixture attributes".to_owned(),
                stats: vec![
                    ListingEquipmentStat {
                        name: "Fixture weight".to_owned(),
                        value: -2.5,
                        base: None,
                    },
                    ListingEquipmentStat {
                        name: "Fixture resistance".to_owned(),
                        value: 12.5,
                        base: Some(10.25),
                    },
                ],
            }],
        )
        .expect("安全装备快照"),
    }
}

fn owned(enabled: bool) -> OwnedListingRecord {
    let created_at = time("2026-11-20T18:00:00Z");
    let expires_at = time("2026-11-21T18:00:00Z");
    let (view, actions) = if enabled {
        (
            PrivateListingViewKind::Resolving(
                ListingLookupProgress::new(0, Some(created_at), None, false, expires_at)
                    .expect("解析进度"),
            ),
            vec![SellerListingAction::Reprice, SellerListingAction::Withdraw],
        )
    } else {
        (
            PrivateListingViewKind::Closed {
                reason: ListingReason::ServiceDisabled,
                equipment: None,
            },
            vec![],
        )
    };
    OwnedListingRecord::new(
        PrivateListing {
            listing_id: LISTING,
            version: version(1),
            identity: ListingEquipmentIdentity::new("main", "42").expect("装备身份"),
            price: price(101),
            pricing_version: 1,
            fee_bps: FeeBps::DEFAULT_CONSIGNED,
            created_at,
            updated_at: created_at,
            expires_at,
            view: PrivateListingView::new(ListingMode::Consigned, ListingState::Resolving, view)
                .expect("挂单视图"),
        },
        actions,
    )
    .expect("本人记录")
}

fn assert_keys(value: &Value, expected: &[&str]) {
    let object = value.as_object().expect("公开 JSON 对象");
    assert_eq!(object.len(), expected.len());
    assert!(expected.iter().all(|key| object.contains_key(*key)), "公开 DTO 字段偏离白名单");
}

fn assert_summary_keys(value: &Value) {
    assert_keys(
        value,
        &[
            "listing_id",
            "version",
            "mode",
            "state",
            "world_id",
            "external_equipment_id",
            "name",
            "category",
            "level",
            "price",
            "seller_name",
            "updated_at",
            "expires_at",
            "purchasable",
        ],
    );
}

#[tokio::test]
async fn status_is_anonymous_and_uses_only_enabled_dependencies() {
    let backend = Arc::new(Backend::default());
    for enabled in [false, true] {
        let result = send(&router(setup(enabled, &backend)), get(STATUS)).await;
        result.assert(200, "ok");
        let fixture = if enabled {
            "public/consignment-status"
        } else {
            "public/consignment-status-disabled"
        };
        assert_wire(fixture, result.body);
        assert_eq!(backend.take_calls(), if enabled { vec![Call::Availability] } else { vec![] });
    }

    for availability in [
        Err(ConsignmentError::Unavailable),
        Ok(ListingRegistrationAvailability::Unavailable),
        Ok(ListingRegistrationAvailability::Ready(ListingWorlds::default())),
    ] {
        let backend = Arc::new(Backend { availability, ..Backend::default() });
        let reply = send(&router(setup(true, &backend)), get(STATUS)).await;
        reply.assert(200, "ok");
        assert_wire("public/consignment-status-unavailable", reply.body);
        assert_eq!(backend.take_calls(), vec![Call::Availability]);
    }

    let backend = Arc::new(Backend {
        availability: Ok(ListingRegistrationAvailability::Ready(ListingWorlds {
            main: false,
            isekai: true,
        })),
        ..Backend::default()
    });
    let app = router(setup(true, &backend));
    let reply = send(&app, get(STATUS)).await;
    assert_eq!(reply.body["data"]["registration_worlds"], serde_json::json!(["isekai"]));
    assert_eq!(backend.take_calls(), vec![Call::Availability]);

    for missing in ["read", "crypto", "sealer", "register"] {
        let mut state = setup(true, &backend);
        match missing {
            "read" => state.capabilities.public_read = None,
            "crypto" => state.capabilities.replay_crypto = None,
            "sealer" => state.capabilities.registration_sealer_available = false,
            "register" => state.capabilities.register = None,
            _ => panic!("未知夹具"),
        }
        let reply = send(&router(state), get(STATUS)).await;
        assert_wire("public/consignment-status-unavailable", reply.body);
        assert_eq!(
            backend.take_calls().len(),
            usize::from(matches!(missing, "sealer" | "register"))
        );
    }
}

#[tokio::test]
async fn status_reports_configured_fee_in_both_switch_states() {
    let backend = Arc::new(Backend::default());
    for enabled in [false, true] {
        let mut state = setup(enabled, &backend);
        state.capabilities.fee_bps = FeeBps::new(615).expect("配置费率");
        let reply = send(&router(state), get(STATUS)).await;
        reply.assert(200, "ok");
        assert_eq!(reply.body["data"]["fee_bps"], 615);
    }
}

#[tokio::test]
async fn status_degrades_when_registration_availability_stalls() {
    let backend = Arc::new(Backend { availability_stalls: true, ..Backend::default() });
    // 给 Tokio 调度留余量。若路由的一秒预算失效，外层边界会先返回 504。
    let boundary = HttpBoundary::new(Duration::from_millis(1250), &[], false).expect("请求边界");
    let app = router_with_boundary(setup(true, &backend), boundary);
    let reply = send(&app, get(STATUS)).await;
    reply.assert(200, "ok");
    assert_wire("public/consignment-status-unavailable", reply.body);
    assert_eq!(backend.take_calls(), vec![Call::Availability]);

    let boundary = HttpBoundary::new(Duration::from_millis(10), &[], false).expect("请求边界");
    let app = router_with_boundary(setup(true, &backend), boundary);
    send(&app, get(STATUS)).await.assert(504, "timeout");
    assert_eq!(backend.take_calls(), vec![Call::Availability]);
}

#[tokio::test]
async fn public_reads_respect_gate_and_output_allowlist() {
    for (consigned, fixture) in [
        (false, "public/consignment-listing-detail"),
        (true, "public/consignment-listing-detail-awaiting-deposit"),
    ] {
        let detail = detail(consigned);
        let listing_id = detail.summary.listing_id;
        let backend = Arc::new(Backend { public: Some(detail), ..Backend::default() });
        let uri = format!("{LISTINGS}/{listing_id}");
        for enabled in [false, true] {
            let app = router(setup(enabled, &backend));
            let page = send(&app, get(LISTINGS)).await;
            let detail = send(&app, get(&uri)).await;
            if enabled {
                page.assert(200, "ok");
                detail.assert(200, "ok");
                assert_keys(&page.body["data"], &["items", "total", "page", "page_size"]);
                assert_eq!(page.body["data"]["total"], 1);
                let data = &detail.body["data"];
                assert_keys(data, &["summary", "equipment"]);
                assert_summary_keys(&data["summary"]);
                assert_summary_keys(&page.body["data"]["items"][0]);
                assert_eq!(data["summary"], page.body["data"]["items"][0]);
                assert_keys(
                    &data["equipment"],
                    &[
                        "name",
                        "alt_name",
                        "category",
                        "level",
                        "tradeable",
                        "stat_groups",
                        "source",
                        "observed_at",
                    ],
                );
                assert_keys(&data["equipment"]["stat_groups"][0], &["name", "stats"]);
                assert_keys(
                    &data["equipment"]["stat_groups"][0]["stats"][0],
                    &["name", "value", "base"],
                );
                assert_wire(fixture, detail.body);
                let calls = backend.take_calls();
                assert!(
                    matches!(calls.as_slice(), [Call::PublicList(_), Call::PublicDetail(id)] if *id == listing_id)
                );
            } else {
                page.assert(404, "consignment_disabled");
                detail.assert(404, "consignment_disabled");
                assert!(backend.take_calls().is_empty());
            }
        }
    }
}

#[tokio::test]
async fn invisible_detail_and_missing_dependencies_are_distinct() {
    let backend = Arc::new(Backend::default());
    let app = router(setup(true, &backend));
    let uri = format!("{LISTINGS}/{LISTING}");
    let missing = send(&app, get(&uri)).await;
    missing.assert(404, "listing_not_found");
    assert_eq!(backend.take_calls(), vec![Call::PublicDetail(LISTING)]);
    for hidden in [fixture_uuid(20), fixture_uuid(21), fixture_uuid(22)] {
        let reply = send(&app, get(&format!("{LISTINGS}/{hidden}"))).await;
        assert_eq!(reply.body, missing.body);
    }

    let mut state = setup(true, &backend);
    state.capabilities.public_read = None;
    let no_port = router(state);
    let failed = Arc::new(Backend {
        read_error: Some(ConsignmentError::CommandNotFound),
        ..Backend::default()
    });
    let bad_port = router(setup(true, &failed));
    for app in [no_port, bad_port] {
        send(&app, get(LISTINGS)).await.assert(503, "consignment_unavailable");
        send(&app, get(&uri)).await.assert(503, "consignment_unavailable");
    }

    let mut skeleton = setup(false, &backend);
    skeleton.capabilities = ConsignmentCapabilities::default();
    let app = router(skeleton);
    send(&app, get(STATUS)).await.assert(200, "ok");
    send(&app, get(&uri)).await.assert(404, "consignment_disabled");
}

#[tokio::test]
async fn query_rejects_all_invalid_values_before_a_reader_even_when_disabled() {
    let backend = Arc::new(Backend::default());
    let long_category = format!("category={}", "%E6%B0%B4".repeat(43));
    let long_q = format!("q={}", "n".repeat(81));
    for enabled in [false, true] {
        let app = router(setup(enabled, &backend));
        for query in [
            "world=MAIN",
            "world=",
            "world=other",
            "mode=other",
            "mode=",
            "world=main&world=isekai",
            "mode=direct&m%6Fde=consigned",
            "unknown=x",
            "state=resolving",
            "category=",
            "category=%20Cloth",
            "category=Cloth%0A",
            "category=Cloth,",
            "category=Cloth,,Staff",
            "category=a,b,c,d,e,f,g,h",
            "category=%E2%80%A8x",
            &long_category,
            "q=",
            "q=+name",
            "q=name%09",
            "q=x%E2%80%A8y",
            "q=x%FF",
            "q=x%Q1",
            "q=x%",
            &long_q,
            "page=0",
            "page=-1",
            "page=1.5",
            "page=one",
            "page=",
            "page=18446744073709551615",
            "page=2147483647&page_size=200",
            "page=52&page_size=200",
            "page=10002&page_size=1",
            "page_size=0",
            "page_size=-1",
            "page_size=1.5",
            "page_size=201",
            "page_size=",
            "page_size=9223372036854775808",
            "page=1&page=2",
            "page_size=1&page_size=20",
        ] {
            send(&app, get(&format!("{LISTINGS}?{query}"))).await.assert(400, "invalid_query");
            assert!(backend.take_calls().is_empty(), "非法公开查询调用了端口");
        }
        for query in [
            "category=Cloth",
            "q=name",
            "state=active",
            "state=",
            "state=Displayed",
            "state=closed&state=rejected",
            "world=other",
            "mode=other",
            "page=0",
            "page_size=201",
            "page=52&page_size=200",
            "unknown=1",
        ] {
            let mut req = get(&format!("{OWNED}?{query}"));
            login(&mut req, SELLER);
            send(&app, req).await.assert(400, "invalid_query");
            assert!(backend.take_calls().is_empty(), "非法本人查询调用了端口");
        }
    }
}

#[tokio::test]
async fn page_limits_and_literal_name_query_reach_the_correct_port() {
    let backend = Arc::new(Backend { page_total: Some(11), ..Backend::default() });
    let app = router(setup(true, &backend));
    for (query, page, size, offset) in [
        ("", 1, 20, 0),
        ("?page_size=1", 1, 1, 0),
        ("?page_size=37", 1, 37, 0),
        ("?page_size=200", 1, 200, 0),
        ("?page=51&page_size=200", 51, 200, 10_000),
        ("?page=10001&page_size=1", 10_001, 1, 10_000),
    ] {
        for path in [LISTINGS, OWNED] {
            let mut req = get(&format!("{path}{query}"));
            if path == OWNED {
                login(&mut req, SELLER);
            }
            let result = send(&app, req).await;
            result.assert(200, "ok");
            assert_eq!(
                result.body["data"],
                serde_json::json!({"page": page, "page_size":size,"items":[],"total":11})
            );
            let calls = backend.take_calls();
            match calls.as_slice() {
                [Call::PublicList(q)] if path == LISTINGS => assert_eq!(q.window.offset(), offset),
                [Call::OwnedList(seller, q)] if path == OWNED => {
                    assert_eq!(seller.as_uuid(), SELLER);
                    assert_eq!(q.window.offset(), offset);
                }
                _ => panic!("分页调用走错端口"),
            }
        }
    }

    let category = "%E6%B0%B4".repeat(42) + "ab";
    // 整串 80 个字符、两段各不超过 128 字节，按空白拆成名称片段。
    let q = "%E6%B0%B4".repeat(40) + "%20" + &"%E6%B0%B4".repeat(36) + "A%25_";
    send(&app, get(&format!("{LISTINGS}?world=isekai&mode=consigned&category={category}&q={q}")))
        .await
        .assert(200, "ok");
    let calls = backend.take_calls();
    let [Call::PublicList(query)] = calls.as_slice() else { panic!("目录筛选未到端口") };
    assert_eq!(query.world, Some(ListingWorld::Isekai));
    assert_eq!(query.mode, Some(ListingMode::Consigned));
    assert_eq!(query.category.as_ref().expect("分类").as_slice(), ["水".repeat(42) + "ab"]);
    assert_eq!(
        query.name.as_ref().expect("名称片段").terms(),
        ["水".repeat(40), "水".repeat(36) + "A%_"]
    );

    send(&app, get(&format!("{LISTINGS}?category=Cloth,Staff,Cloth"))).await.assert(200, "ok");
    let calls = backend.take_calls();
    let [Call::PublicList(query)] = calls.as_slice() else { panic!("多分类未到端口") };
    assert_eq!(query.category.as_ref().expect("分类").as_slice(), ["Cloth", "Staff"]);

    let mut req = get(&format!("{OWNED}?world=main&mode=direct&state=expired"));
    login(&mut req, SELLER);
    send(&app, req).await.assert(200, "ok");
    assert!(matches!(backend.take_calls().as_slice(), [Call::OwnedList(_, q)]
        if q.state == Some(ListingState::Expired) && q.mode == Some(ListingMode::Direct)
           && q.world == Some(ListingWorld::Main)));
}

#[tokio::test]
async fn empty_query_routes_and_canonical_paths_are_checked_before_dependency() {
    let backend = Arc::new(Backend::default());
    for enabled in [false, true] {
        let app = router(setup(enabled, &backend));
        for path in [
            STATUS.to_owned(),
            format!("{LISTINGS}/{LISTING}"),
            format!("{OWNED}/{LISTING}"),
            format!("{COMMANDS}/{}", fixture_uuid(0x65)),
        ] {
            let mut req = get(&format!("{path}?unexpected=1"));
            login(&mut req, SELLER);
            send(&app, req).await.assert(400, "invalid_query");
        }
        for raw in [
            "bad-id",
            "00000000-0000-0000-0000-000000000000",
            "00000000-0000-4000-8000-0000000000FA",
            "000000000000400080000000000000fa",
            "%FF",
        ] {
            for (path, code) in [
                (LISTINGS, "invalid_listing_id"),
                (OWNED, "invalid_listing_id"),
                (COMMANDS, "invalid_command_id"),
            ] {
                let mut req = get(&format!("{path}/{raw}"));
                login(&mut req, SELLER);
                send(&app, req).await.assert(400, code);
            }
        }
        assert!(backend.take_calls().is_empty());
    }
}

#[tokio::test]
async fn owned_reads_keep_history_and_effective_state_when_closed() {
    for enabled in [false, true] {
        let backend = Arc::new(Backend { owned: Some(owned(enabled)), ..Backend::default() });
        let app = router(setup(enabled, &backend));
        let expected_state = if enabled { "resolving" } else { "closed" };
        let mut list = get(&format!("{OWNED}?state={expected_state}"));
        login(&mut list, SELLER);
        let page = send(&app, list).await;
        page.assert(200, "ok");
        let mut request = get(&format!("{OWNED}/{LISTING}"));
        login(&mut request, SELLER);
        let detail = send(&app, request).await;
        detail.assert(200, "ok");
        assert_eq!(page.body["data"]["items"][0], detail.body["data"]);
        assert_eq!(page.body["data"]["total"], 1);
        if enabled {
            assert_wire("public/consignment-owned-listing", detail.body);
        } else {
            assert_eq!(detail.body["data"]["stored_state"], "resolving");
            assert_eq!(detail.body["data"]["effective_state"], "closed");
            assert_eq!(detail.body["data"]["effective_reason"], "service_disabled");
            assert_eq!(detail.body["data"]["version"], 1);
            assert_eq!(detail.body["data"]["allowed_actions"], serde_json::json!([]));
            let mut stored_only = get(&format!("{OWNED}?state=resolving"));
            login(&mut stored_only, SELLER);
            let filtered = send(&app, stored_only).await;
            filtered.assert(200, "ok");
            assert_eq!(filtered.body["data"]["items"], serde_json::json!([]));
        }
        let calls = backend.take_calls();
        let [Call::OwnedList(seller, query), Call::OwnedDetail(owner, listing_id), ..] =
            calls.as_slice()
        else {
            panic!("缺少本人读取调用");
        };
        assert_eq!(
            (*seller, *owner, *listing_id),
            (UserId::from_uuid(SELLER), UserId::from_uuid(SELLER), LISTING)
        );
        assert_eq!(query.state.map(ListingState::as_str), Some(expected_state));
    }
}

#[tokio::test]
async fn owned_identity_precedes_query_and_path_and_missing_is_indistinguishable() {
    for enabled in [false, true] {
        let session = Arc::new(Session::default());
        let command_id = CommandId::from_uuid(fixture_uuid(0x65));
        let backend = Arc::new(Backend {
            owned: Some(owned(enabled)),
            command: Some(OwnedListingCommandReceipt {
                command_id,
                decision: OwnedListingDecision::Register(StoredListingDecision::Accepted(
                    ListingRegistered::new(LISTING, price(101)).expect("登记回执"),
                )),
            }),
            ..Backend::default()
        });
        let app = router(configured(enabled, &backend, Some(session.clone()), 100));
        for uri in [
            format!("{OWNED}?unknown=1"),
            format!("{OWNED}/invalid"),
            format!("{COMMANDS}/invalid"),
        ] {
            send(&app, get(&uri)).await.assert(401, "unauthenticated");
            assert_eq!(session.take_calls(), 0);
            let mut expired = get(&uri);
            expired.headers_mut().insert(header::COOKIE, HeaderValue::from_static(EXPIRED_COOKIE));
            send(&app, expired).await.assert(401, "unauthenticated");
            assert_eq!(session.take_calls(), 1);
            for (expected, status, code) in [
                ("invalid".to_owned(), 400, "invalid_expected_user_id"),
                (OTHER_SELLER.to_string(), 409, "session_changed"),
            ] {
                let mut req = get(&uri);
                login(&mut req, SELLER);
                req.headers_mut().insert(
                    PUBLIC_EXPECTED_USER_ID_HEADER,
                    HeaderValue::from_str(&expected).expect("请求头"),
                );
                send(&app, req).await.assert(status, code);
                assert_eq!(session.take_calls(), 1);
            }
            let mut duplicate = get(&uri);
            login(&mut duplicate, SELLER);
            duplicate
                .headers_mut()
                .append(PUBLIC_EXPECTED_USER_ID_HEADER, HeaderValue::from_static("duplicate"));
            send(&app, duplicate).await.assert(400, "invalid_expected_user_id");
            assert_eq!(session.take_calls(), 1);
            assert!(backend.take_calls().is_empty());
        }

        let mut other_list = get(OWNED);
        login(&mut other_list, OTHER_SELLER);
        let response = send(&app, other_list).await;
        response.assert(200, "ok");
        assert_eq!(response.body["data"]["items"], serde_json::json!([]));
        for (path, real, absent, code) in [
            (OWNED, LISTING, fixture_uuid(91), "listing_not_found"),
            (COMMANDS, command_id.as_uuid(), fixture_uuid(92), "command_not_found"),
        ] {
            let mut other = get(&format!("{path}/{real}"));
            login(&mut other, OTHER_SELLER);
            let other = send(&app, other).await;
            let mut unknown = get(&format!("{path}/{absent}"));
            login(&mut unknown, SELLER);
            let unknown = send(&app, unknown).await;
            other.assert(404, code);
            unknown.assert(404, code);
            assert_eq!(other.body, unknown.body);
        }
    }
}

#[tokio::test]
async fn stored_command_returns_the_original_status_under_get_200() {
    for (command_uuid, decision, fixture) in [
        (
            fixture_uuid(0x65),
            StoredListingDecision::Accepted(
                ListingRegistered::new(LISTING, price(101)).expect("登记回执"),
            ),
            "public/consignment-command-result",
        ),
        (
            fixture_uuid(0x6a),
            StoredListingDecision::Rejected(ListingBusinessRejection::FeeChanged),
            "public/consignment-command-rejected",
        ),
    ] {
        let command_id = CommandId::from_uuid(command_uuid);
        let backend = Arc::new(Backend {
            command: Some(OwnedListingCommandReceipt {
                command_id,
                decision: OwnedListingDecision::Register(decision),
            }),
            ..Backend::default()
        });
        for enabled in [false, true] {
            let mut req = get(&format!("{COMMANDS}/{command_uuid}"));
            login(&mut req, SELLER);
            let reply = send(&router(setup(enabled, &backend)), req).await;
            reply.assert(200, "ok");
            assert_wire(fixture, reply.body);
            assert_eq!(
                backend.take_calls(),
                vec![Call::OwnedCommand(UserId::from_uuid(SELLER), command_id)]
            );
        }
    }
}

#[tokio::test]
async fn stored_command_recovers_reprice_and_withdraw_in_both_switch_states() {
    for (command_uuid, decision, fixture) in [
        (
            fixture_uuid(0x66),
            OwnedListingDecision::Reprice(StoredListingDecision::Accepted(
                ListingPriceReceipt::new(
                    LISTING,
                    version(2),
                    ListingState::Resolving,
                    price(102),
                    ListingPriceDisposition::Repriced,
                )
                .expect("改价回执"),
            )),
            "public/consignment-reprice-receipt",
        ),
        (
            fixture_uuid(0x68),
            OwnedListingDecision::Withdraw(StoredListingDecision::Accepted(
                ListingClosed::new(LISTING, version(3), Some(price(102))).expect("取消回执"),
            )),
            "public/consignment-withdraw-receipt",
        ),
    ] {
        let command_id = CommandId::from_uuid(command_uuid);
        let backend = Arc::new(Backend {
            command: Some(OwnedListingCommandReceipt { command_id, decision }),
            ..Backend::default()
        });
        for enabled in [true, false] {
            let mut req = get(&format!("{COMMANDS}/{command_uuid}"));
            login(&mut req, SELLER);
            let reply = send(&router(setup(enabled, &backend)), req).await;
            reply.assert(200, "ok");
            assert_eq!(
                reply.body["data"],
                serde_json::json!({
                    "http_status": 200,
                    "code": "ok",
                    "receipt": wire_fixture(fixture)["data"],
                })
            );
            assert_eq!(
                backend.take_calls(),
                vec![Call::OwnedCommand(UserId::from_uuid(SELLER), command_id)]
            );
        }
    }
}

#[tokio::test]
async fn stored_command_rejects_receipt_for_another_command_id() {
    let command_uuid = fixture_uuid(0x65);
    let other_command = fixture_uuid(0x66);
    let backend = Arc::new(Backend {
        command: Some(OwnedListingCommandReceipt {
            command_id: CommandId::from_uuid(other_command),
            decision: OwnedListingDecision::Register(StoredListingDecision::Accepted(
                ListingRegistered::new(LISTING, price(101)).expect("登记回执"),
            )),
        }),
        return_wrong_command_id: true,
        ..Backend::default()
    });
    for enabled in [true, false] {
        let mut req = get(&format!("{COMMANDS}/{command_uuid}"));
        login(&mut req, SELLER);
        let reply = send(&router(setup(enabled, &backend)), req).await;
        reply.assert(503, "consignment_unavailable");
        assert!(!reply.body.to_string().contains(&other_command.to_string()));
        assert_eq!(
            backend.take_calls(),
            vec![Call::OwnedCommand(UserId::from_uuid(SELLER), CommandId::from_uuid(command_uuid))]
        );
    }
}

#[tokio::test]
async fn private_dependency_failure_is_503_in_both_switch_states() {
    let backend = Arc::new(Backend::default());
    let failed =
        Arc::new(Backend { read_error: Some(ConsignmentError::Unavailable), ..Backend::default() });
    for enabled in [false, true] {
        let mut state = setup(enabled, &backend);
        state.capabilities.owned_read = None;
        let no_port = router(state);
        let failed_port = router(setup(enabled, &failed));
        let no_session = router(configured(enabled, &backend, None, 100));
        for uri in [OWNED.to_owned(), format!("{OWNED}/{LISTING}"), format!("{COMMANDS}/{LISTING}")]
        {
            for (app, code) in [
                (&no_port, "consignment_unavailable"),
                (&failed_port, "consignment_unavailable"),
                (&no_session, "storage_unavailable"),
            ] {
                let mut req = get(&uri);
                login(&mut req, SELLER);
                send(app, req).await.assert(503, code);
            }
        }
        assert!(backend.take_calls().is_empty());
        assert_eq!(failed.take_calls().len(), 3);
    }
}

#[tokio::test]
async fn inconsistent_page_detail_or_closed_projection_fails_without_partial_data() {
    let record = detail(false);
    let uri = format!("{LISTINGS}/{}", record.summary.listing_id);
    let invalid = [
        Backend { public: Some(record.clone()), page_total: Some(0), ..Backend::default() },
        Backend { public: Some(record.clone()), page_total: Some(u64::MAX), ..Backend::default() },
        Backend {
            public: Some(record.clone()),
            window_override: Some(ListingPageWindow::new(Some(2), None).expect("页")),
            ..Backend::default()
        },
    ];
    for backend in invalid {
        send(&router(setup(true, &Arc::new(backend))), get(LISTINGS))
            .await
            .assert(503, "consignment_unavailable");
    }

    let mut wrong_name = record.clone();
    wrong_name.summary.name = "Inconsistent name".to_owned();
    let mut wrong_category = record.clone();
    wrong_category.summary.category = "Inconsistent category".to_owned();
    let mut wrong_level = record.clone();
    wrong_level.summary.level = ListingEquipmentLevel::Assigned(43);
    let mut wrong_id = record;
    wrong_id.summary.listing_id = LISTING;
    for invalid in [wrong_name, wrong_category, wrong_level, wrong_id] {
        let backend = Arc::new(Backend { public: Some(invalid), ..Backend::default() });
        send(&router(setup(true, &backend)), get(&uri))
            .await
            .assert(503, "consignment_unavailable");
    }

    let stale = Arc::new(Backend { owned: Some(owned(true)), ..Backend::default() });
    let app = router(setup(false, &stale));
    for uri in [OWNED.to_owned(), format!("{OWNED}/{LISTING}")] {
        let mut req = get(&uri);
        login(&mut req, SELLER);
        send(&app, req).await.assert(503, "consignment_unavailable");
    }
}

#[tokio::test]
async fn methods_are_strict_and_head_never_calls_a_port() {
    let backend = Arc::new(Backend::default());
    let app = router(setup(true, &backend));
    let head = send(&app, request(Method::HEAD, LISTINGS, Body::empty())).await;
    assert_eq!(head.status, 405);
    assert_eq!(head.headers[header::ALLOW], "GET, POST");
    send(&app, request(Method::DELETE, LISTINGS, Body::empty()))
        .await
        .assert(405, "method_not_allowed");
    assert!(backend.take_calls().is_empty());
}

#[tokio::test]
async fn malformed_secret_query_never_appears_in_an_error_response() {
    let backend = Arc::new(Backend::default());
    let app = router(setup(true, &backend));
    let encoded: String = url::form_urlencoded::byte_serialize(FAKE_LINK.as_bytes()).collect();
    assert!(encoded.contains(FAKE_KEY), "夹具必须包含假 key");
    send(&app, get(&format!("{LISTINGS}?unknown={encoded}"))).await.assert(400, "invalid_query");
    assert!(backend.take_calls().is_empty());
}

#[tokio::test]
async fn mounted_routes_set_no_store_before_the_global_boundary() {
    let backend = Arc::new(Backend::default());
    let app = mount(axum::Router::new()).with_state(setup(true, &backend));
    for uri in [STATUS, LISTINGS, OWNED] {
        let response = app.clone().oneshot(get(uri)).await.expect("挂载路由响应");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}

#[tokio::test]
async fn owned_detail_serializes_every_view_variant_with_the_shared_contract() {
    let direct = detail(false);
    let consigned = detail(true);
    let observed_at = time("2026-11-20T18:10:00Z");
    let bound = ListingEquipmentDetails::new(
        observed_at,
        ListingEquipmentLabel {
            tradeable: false,
            level: ListingEquipmentLevel::Soulbound,
            ..direct.equipment.label().clone()
        },
        direct.equipment.stat_groups().to_vec(),
    )
    .expect("不可交易快照");

    let cases = [
        (
            2,
            "main",
            "42",
            ListingMode::Direct,
            ListingState::Rejected,
            "2026-11-20T18:00:00Z",
            "2026-11-20T18:15:00Z",
            "2026-11-21T18:00:00Z",
            PrivateListingViewKind::LookupFailed {
                reason: ListingReason::LookupExhausted,
                last_lookup_reason: Some(LookupTransientReason::SourceTimeout),
            },
        ),
        (
            3,
            "main",
            "42",
            ListingMode::Consigned,
            ListingState::Rejected,
            "2026-11-20T18:00:00Z",
            "2026-11-20T18:14:00Z",
            "2026-11-21T18:00:00Z",
            PrivateListingViewKind::Ineligible {
                equipment: bound,
                reason: ListingReason::NotTradeable,
            },
        ),
        (
            4,
            "main",
            "42424242424242424242424242424242",
            ListingMode::Direct,
            ListingState::Displayed,
            "2026-11-20T18:00:00Z",
            "2026-11-20T18:13:00Z",
            "2026-12-20T18:10:00Z",
            PrivateListingViewKind::Displayed(direct.equipment.clone()),
        ),
        (
            5,
            "isekai",
            "43",
            ListingMode::Consigned,
            ListingState::AwaitingDeposit,
            "2026-11-20T18:00:00Z",
            "2026-11-20T18:12:00Z",
            "2026-11-27T18:10:00Z",
            PrivateListingViewKind::AwaitingDeposit(consigned.equipment),
        ),
        (
            6,
            "main",
            "42",
            ListingMode::Direct,
            ListingState::Displayed,
            "2026-11-20T18:00:00Z",
            "2026-11-20T18:11:00Z",
            "2026-12-20T18:10:00Z",
            PrivateListingViewKind::Closed {
                reason: ListingReason::AuctionLotCreated,
                equipment: Some(direct.equipment),
            },
        ),
        (
            7,
            "main",
            "42",
            ListingMode::Direct,
            ListingState::Expired,
            "2026-11-19T18:10:00Z",
            "2026-11-20T18:10:00Z",
            "2026-11-20T18:10:00Z",
            PrivateListingViewKind::Expired {
                reason: ListingReason::LookupDeadline,
                equipment: None,
            },
        ),
    ];

    let fixture = wire_fixture("public/consignment-owned-listings");
    let expected = fixture["data"]["items"].as_array().expect("本人列表夹具");
    let records = cases.into_iter().enumerate().map(
        |(index, (id, world, eid, mode, stored, created_at, updated_at, expires_at, view))| {
            let view = PrivateListingView::new(mode, stored, view).expect("挂单视图");
            let actions = if matches!(id, 4 | 5) {
                vec![SellerListingAction::Reprice, SellerListingAction::Withdraw]
            } else {
                vec![]
            };
            let owned = OwnedListingRecord::new(
                PrivateListing {
                    listing_id: fixture_uuid(id),
                    version: version(2),
                    identity: ListingEquipmentIdentity::new(world, eid).expect("装备身份"),
                    price: price(101),
                    pricing_version: 1,
                    fee_bps: FeeBps::for_mode(mode, FeeBps::DEFAULT_CONSIGNED),
                    created_at: time(created_at),
                    updated_at: time(updated_at),
                    expires_at: time(expires_at),
                    view,
                },
                actions,
            )
            .expect("本人记录");
            (owned, &expected[index])
        },
    );
    for (record, expected) in records.chain([(owned(true), &expected[6])]) {
        let listing_id = record.listing().listing_id;
        let backend = Arc::new(Backend { owned: Some(record), ..Backend::default() });
        let app = router(setup(true, &backend));
        let mut req = get(&format!("{OWNED}/{listing_id}"));
        login(&mut req, SELLER);
        let reply = send(&app, req).await;
        reply.assert(200, "ok");
        assert_eq!(&reply.body["data"], expected);
    }
}
