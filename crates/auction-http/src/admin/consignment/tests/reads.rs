//! Admin 历史、闭集分页与详情投影的 HTTP 行为。

use auction_application::consignment::{
    AdminListingDetail, AdminListingQuery, AdminListingRecord, ConsignmentError,
    ListingAuctionConflict, ListingDelistReason, ListingEquipmentDetails, ListingEquipmentLabel,
    ListingEquipmentLevel, ListingEquipmentStat, ListingEquipmentStatGroup, ListingPageWindow,
    ListingWorld, LookupTransientReason, PrivateListingQuery, PrivateListingView,
    PrivateListingViewKind,
};
use auction_domain::consignment::{ListingMode, ListingReason, ListingState};
use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::fixture::{
    Fixture, LIST_PATH, LISTING, REASON, ReadCall, SELLER, detail_path, failure, get, response,
};
use crate::server::contract_fixtures::time;

#[tokio::test]
async fn lists_and_reads_without_snapshot_when_enabled_or_disabled() {
    for (enabled, expected) in [(true, "resolving"), (false, "closed")] {
        let fixture = Fixture::new(enabled);
        let list = response(get(&fixture, LIST_PATH).await, StatusCode::OK).await;
        let detail = response(get(&fixture, &detail_path()).await, StatusCode::OK).await;
        assert_eq!(list["code"], "ok");
        assert_eq!(list["data"]["total"], 1);
        assert_eq!(list["data"]["page"], 1);
        assert_eq!(list["data"]["page_size"], 20);
        let record = &list["data"]["items"][0];
        assert_eq!(record, &detail["data"]["record"]);
        assert_eq!(record["seller_user_id"], SELLER.to_string());
        assert_eq!(record["seller_name"], "fixture-seller");
        assert_eq!(record["can_delist"], enabled);
        assert_eq!(record["listing"]["listing_id"], LISTING.to_string());
        assert_eq!(record["listing"]["external_equipment_id"], "42");
        assert_eq!(record["listing"]["stored_state"], "resolving");
        assert_eq!(record["listing"]["effective_state"], expected);
        assert_eq!(record["listing"]["version"], 7);
        assert_eq!(record["listing"]["allowed_actions"], json!([]));
        assert_eq!(record["listing"]["estimated_fee"], 5);
        assert_eq!(record["listing"]["estimated_seller_amount"], 96);
        assert_eq!(record["listing"]["view"]["kind"], expected);
        if enabled {
            assert_eq!(record["listing"]["view"]["progress"]["max_attempts"], 6);
            assert_eq!(record["listing"]["view"]["progress"]["attempts"], 0);
        } else {
            assert_eq!(record["listing"]["effective_reason"], "service_disabled");
            assert!(record["listing"]["view"].get("equipment").is_some_and(Value::is_null));
        }
        assert_eq!(detail["data"]["conflicts"], json!([]));
        assert_eq!(detail["data"]["conflicts_truncated"], false);
        assert_eq!(fixture.read_port().calls.lock().unwrap()[1], ReadCall::Detail(LISTING));
    }
}

#[tokio::test]
async fn list_filters_forward_effective_state_and_pagination_accepts_empty_pages() {
    let fixture = Fixture::new(false);
    let path = format!(
        "{LIST_PATH}?world=isekai&mode=consigned&state=closed&seller_user_id={SELLER}&page=2&page_size=37"
    );
    let reply = response(get(&fixture, &path).await, StatusCode::OK).await;
    assert_eq!(reply["data"], json!({"items": [], "total": 1, "page": 2, "page_size": 37}));
    assert_eq!(
        fixture.read_port().calls.lock().unwrap().as_slice(),
        &[ReadCall::List(AdminListingQuery {
            listings: PrivateListingQuery {
                window: ListingPageWindow::new(Some(2), Some(37)).unwrap(),
                world: Some(ListingWorld::Isekai),
                mode: Some(ListingMode::Consigned),
                state: Some(ListingState::Closed),
            },
            seller_user_id: Some(SELLER.into()),
        })],
    );
    for (query, page, size, offset) in [
        ("page=10001&page_size=1", 10_001, 1, 10_000),
        ("page=51&page_size=200", 51, 200, 10_000),
        ("page_size=37", 1, 37, 0),
        ("page=2", 2, 20, 20),
    ] {
        let path = format!("{LIST_PATH}?{query}");
        let reply = response(get(&fixture, &path).await, StatusCode::OK).await;
        assert_eq!(reply["data"]["page"], page);
        assert_eq!(reply["data"]["page_size"], size);
        let calls = fixture.read_port().calls.lock().unwrap();
        let ReadCall::List(query) = calls.last().unwrap() else {
            panic!("列表必须调用列表端口");
        };
        assert_eq!(query.listings.window.offset(), offset);
    }
}

#[tokio::test]
async fn each_invalid_duplicate_or_unknown_filter_fails_before_port_selection() {
    let fixture = Fixture::new(true);
    let valid = [
        ("world", "main"),
        ("mode", "direct"),
        ("state", "displayed"),
        ("seller_user_id", "00000000-0000-0000-0000-00000000abcd"),
        ("page", "1"),
        ("page_size", "37"),
    ];
    for (key, good) in valid {
        for query in [format!("{key}="), format!("{key}={good}&{key}={good}")] {
            invalid_query(&fixture, &query).await;
        }
    }
    for query in [
        "world=other",
        "world=MAIN",
        "mode=display_only",
        "mode=CONSIGNED",
        "state=active",
        "state=Awaiting_deposit",
        "seller_user_id=alice",
        "seller_user_id=00000000-0000-0000-0000-000000000000",
        "seller_user_id=00000000-0000-0000-0000-00000000ABCD",
        "seller_user_id=0000000000000000000000000000abcd",
        "page=0",
        "page=-1",
        "page=1.5",
        "page=%2B1",
        "page=4294967296",
        "page=4294967295&page_size=200",
        "page_size=0",
        "page_size=-1",
        "page_size=201",
        "page_size=2.0",
        "page_size=1e2",
        "page=52&page_size=200",
        "page=10002&page_size=1",
        "unknown=1",
        "world=main&w%6frld=main",
    ] {
        invalid_query(&fixture, query).await;
    }
    fixture.assert_no_business_calls();
}

async fn invalid_query(fixture: &Fixture, query: &str) {
    let reply = get(fixture, &format!("{LIST_PATH}?{query}")).await;
    failure(reply, StatusCode::BAD_REQUEST, "invalid_query").await;
}

#[tokio::test]
async fn detail_preserves_conflict_order_truncation_and_private_reason() {
    let mut fixture = Fixture::new(false);
    let mut private = fixture.read_port().record.listing().clone();
    private.view = PrivateListingView::new(
        ListingMode::Consigned,
        ListingState::Closed,
        PrivateListingViewKind::Closed { reason: ListingReason::AdminDelisted, equipment: None },
    )
    .unwrap();
    let record = AdminListingRecord::new(
        private,
        SELLER.into(),
        "fixture-seller".to_owned(),
        Some(ListingDelistReason::new(REASON.to_owned()).unwrap()),
        false,
    )
    .unwrap();
    for count in [3, 50] {
        let conflicts: Vec<_> = (1..=count)
            .map(|lot| ListingAuctionConflict {
                lot_id: Uuid::from_u128(lot).into(),
                // 故意把较新记录放第二项，数据库负责排序，HTTP 应保留端口提供的顺序。
                detected_at: time() + auction_time::Duration::seconds(i64::from(lot == 2)),
            })
            .collect();
        fixture.read_port_mut().detail = Ok(Some(AdminListingDetail {
            record: record.clone(),
            conflicts: conflicts.clone(),
            conflicts_truncated: count == 50,
        }));
        let reply = response(get(&fixture, &detail_path()).await, StatusCode::OK).await;
        let detail: auction_contracts::AdminListingDetail =
            serde_json::from_value(reply["data"].clone()).unwrap();
        assert_eq!(detail.record.admin_delist_reason.as_deref(), Some(REASON));
        assert_eq!(
            detail.record.listing.effective_reason,
            Some(auction_contracts::ListingReason::AdminDelisted)
        );
        assert_eq!(detail.conflicts_truncated, count == 50);
        let previous = detail
            .conflicts
            .iter()
            .map(|entry| (entry.lot_id, entry.detected_at))
            .collect::<Vec<_>>();
        let expected = conflicts
            .iter()
            .map(|entry| (entry.lot_id.as_uuid(), entry.detected_at))
            .collect::<Vec<_>>();
        assert_eq!(previous, expected);
    }
}

#[tokio::test]
async fn invalid_collision_shape_fails_closed_instead_of_rewriting_the_read_model() {
    let mut fixture = Fixture::new(false);
    for (count, truncated) in [(49, true), (51, false)] {
        let port = fixture.read_port_mut();
        port.detail = Ok(Some(AdminListingDetail {
            record: port.record.clone(),
            conflicts: (1..=count)
                .map(|lot| ListingAuctionConflict {
                    lot_id: Uuid::from_u128(lot).into(),
                    detected_at: time(),
                })
                .collect(),
            conflicts_truncated: truncated,
        }));
        failure(
            get(&fixture, &detail_path()).await,
            StatusCode::SERVICE_UNAVAILABLE,
            "consignment_unavailable",
        )
        .await;
    }
}

#[tokio::test]
async fn detail_rejects_invalid_target_and_query_and_maps_missing_and_unavailable() {
    for enabled in [true, false] {
        let mut fixture = Fixture::new(enabled);
        for invalid in [
            "bad-id",
            "00000000-0000-0000-0000-00000000%63afe",
            "%ff",
            "00000000-0000-0000-0000-000000000000",
            "00000000-0000-0000-0000-00000000CAFE",
            "0000000000000000000000000000cafe",
        ] {
            let path = format!("{LIST_PATH}/{invalid}");
            failure(get(&fixture, &path).await, StatusCode::BAD_REQUEST, "invalid_listing_id")
                .await;
        }
        let path = format!("{}?page=1", detail_path());
        failure(get(&fixture, &path).await, StatusCode::BAD_REQUEST, "invalid_query").await;
        fixture.assert_no_business_calls();

        fixture.read_port_mut().detail = Ok(None);
        failure(get(&fixture, &detail_path()).await, StatusCode::NOT_FOUND, "listing_not_found")
            .await;
        fixture.read_port_mut().detail = Err(ConsignmentError::Unavailable);
        fixture.read_port_mut().list_error = Some(ConsignmentError::Unavailable);
        for path in [LIST_PATH, &detail_path()] {
            failure(
                get(&fixture, path).await,
                StatusCode::SERVICE_UNAVAILABLE,
                "consignment_unavailable",
            )
            .await;
        }
        fixture.read = None;
        for path in [LIST_PATH, &detail_path()] {
            failure(
                get(&fixture, path).await,
                StatusCode::SERVICE_UNAVAILABLE,
                "consignment_unavailable",
            )
            .await;
        }
    }
}

#[tokio::test]
async fn detail_projects_safe_snapshot_and_active_or_terminal_view_variants() {
    let equipment = ListingEquipmentDetails::new(
        time(),
        ListingEquipmentLabel {
            name: "装备夹具".to_owned(),
            alt_name: Some("装备别名".to_owned()),
            category: "测试分类".to_owned(),
            level: ListingEquipmentLevel::Assigned(42),
            tradeable: true,
        },
        vec![ListingEquipmentStatGroup {
            name: "组".to_owned(),
            stats: vec![
                ListingEquipmentStat { name: "甲".to_owned(), value: 12.5, base: Some(10.0) },
                ListingEquipmentStat { name: "乙".to_owned(), value: 2.0, base: None },
            ],
        }],
    )
    .unwrap();
    let mut fixture = Fixture::new(true);
    for (mode, stored, view, wire_kind, effective) in [
        (
            ListingMode::Direct,
            ListingState::Displayed,
            PrivateListingViewKind::Displayed(equipment.clone()),
            "displayed",
            "displayed",
        ),
        (
            ListingMode::Consigned,
            ListingState::AwaitingDeposit,
            PrivateListingViewKind::AwaitingDeposit(equipment.clone()),
            "awaiting_deposit",
            "awaiting_deposit",
        ),
        (
            ListingMode::Consigned,
            ListingState::Rejected,
            PrivateListingViewKind::Ineligible {
                equipment: equipment.clone(),
                reason: ListingReason::OwnerMismatch,
            },
            "ineligible",
            "rejected",
        ),
        (
            ListingMode::Consigned,
            ListingState::Rejected,
            PrivateListingViewKind::LookupFailed {
                reason: ListingReason::LookupExhausted,
                last_lookup_reason: Some(LookupTransientReason::SourceTimeout),
            },
            "lookup_failed",
            "rejected",
        ),
        (
            ListingMode::Direct,
            ListingState::Displayed,
            PrivateListingViewKind::Expired {
                reason: ListingReason::DisplayExpired,
                equipment: Some(equipment.clone()),
            },
            "expired",
            "expired",
        ),
        (
            ListingMode::Direct,
            ListingState::Displayed,
            PrivateListingViewKind::Closed {
                reason: ListingReason::AuctionLotCreated,
                equipment: Some(equipment.clone()),
            },
            "closed",
            "closed",
        ),
    ] {
        let port = fixture.read_port_mut();
        let mut private = port.record.listing().clone();
        private.view = PrivateListingView::new(mode, stored, view).unwrap();
        if mode == ListingMode::Direct {
            private.fee_bps = auction_domain::consignment::FeeBps::DIRECT;
        }
        port.detail = Ok(Some(AdminListingDetail {
            record: AdminListingRecord::new(
                private,
                SELLER.into(),
                "fixture-seller".to_owned(),
                None,
                false,
            )
            .unwrap(),
            conflicts: Vec::new(),
            conflicts_truncated: false,
        }));
        let reply = response(get(&fixture, &detail_path()).await, StatusCode::OK).await;
        let listing = &reply["data"]["record"]["listing"];
        assert_eq!(listing["stored_state"], stored.as_str());
        assert_eq!(listing["effective_state"], effective);
        assert_eq!(listing["view"]["kind"], wire_kind);
        if wire_kind == "lookup_failed" {
            assert_eq!(listing["view"]["last_lookup_reason"], "source_timeout");
            assert!(listing["view"].get("equipment").is_none());
        } else {
            let wire = &listing["view"]["equipment"];
            assert_eq!(wire["level"], json!({"kind":"assigned","value":42}));
            assert_eq!(wire["source"], "hvdata-equip-v1");
            assert_eq!(wire["alt_name"], "装备别名");
            assert_eq!(
                wire["stat_groups"][0]["stats"],
                json!([
                    {"name":"乙", "value":2.0, "base":null},
                    {"name":"甲", "value":12.5, "base":10.0}
                ])
            );
            assert_eq!(wire.as_object().unwrap().len(), 8);
        }
        if wire_kind == "awaiting_deposit" {
            assert_eq!(listing["view"]["accepts_mail"], false);
        }
    }
}
