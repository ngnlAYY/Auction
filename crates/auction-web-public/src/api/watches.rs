//! 装备标记同源传输；身份、集合完整性和版本在边界严格校验。

use std::collections::BTreeSet;

use auction_contracts::{
    PublicLotWatchResponse, PublicLotWatchesResponse, PublicSetLotWatchRequest,
    PublicSetLotWatchResponse,
};
use auction_web_shared::valid_uuid;
use auction_web_shared::wire::json_has_exact_keys as exact_fields;
use gloo_net::http::Request;
use serde_json::Value;

use super::clock::send_response;
use super::session::identity::{
    SessionIdentity, SessionIdentityObservation, SessionRefreshReason, request_public_session_check,
};
use super::session::transport::{decode_account_identity_response, decode_observed_account_read};
use super::{CommandError, TransportError};

/// 整场集合有界且不能截断；缺项只有在完整权威读取后才表示 false/version 0。
pub(crate) async fn fetch_lot_watches(
    user_id: &str,
    auction_id: &str,
) -> Result<(PublicLotWatchesResponse, SessionIdentityObservation), CommandError> {
    let observation = SessionIdentityObservation::watch_collection();
    #[cfg(test)]
    if let Some(fixture) = crate::surface::PageReadFixture::<
        _,
        Result<PublicLotWatchesResponse, CommandError>,
    >::response((user_id.to_owned(), auction_id.to_owned()))
    {
        return accept_collection(fixture?, observation, user_id, auction_id, 200);
    }
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("auction_id", auction_id)
        .finish();
    let path = format!("/api/v1/me/lot-watches?{query}");
    let response = send_response(
        |abort_signal| {
            Request::get(&path)
                .abort_signal(abort_signal)
                .build()
                .map_err(|_error| TransportError::Network(None))
        },
        #[cfg(test)]
        crate::surface::PageReadFixture::response((user_id.to_owned(), auction_id.to_owned())),
    )
    .await;
    if !observation.current() {
        return Err(failure("session_observation_stale", 0));
    }
    let response = response.map_err(|error| failure(error.code(), 0))?;
    let status = response.status();
    let page: PublicLotWatchesResponse = decode_observed_account_read(
        response,
        || observation.current(),
        |value| {
            exact_fields(value, &["user_id", "auction_id", "watches"])
                && value.get("watches").and_then(Value::as_array).is_some_and(|watches| {
                    watches.len() <= 10_000 && watches.iter().all(valid_watch_value)
                })
        },
    )
    .await?;
    accept_collection(page, observation, user_id, auction_id, status)
}

fn accept_collection(
    page: PublicLotWatchesResponse,
    observation: SessionIdentityObservation,
    user_id: &str,
    auction_id: &str,
    status: u16,
) -> Result<(PublicLotWatchesResponse, SessionIdentityObservation), CommandError> {
    if !valid_watch_collection(&page, &page.user_id, auction_id) {
        return Err(failure("invalid_response", status));
    }
    if !observation.publish(SessionIdentity::UserId(page.user_id.clone())) {
        return Err(failure("session_observation_stale", 0));
    }
    if page.user_id != user_id {
        return Err(failure("invalid_response", status));
    }
    Ok((page, observation))
}

/// 重试直接复用原 DTO，不重新生成 ID、目标状态或 CAS 版本。
pub(crate) async fn set_lot_watch(
    user_id: &str,
    auction_id: &str,
    lot_id: &str,
    command: &PublicSetLotWatchRequest,
) -> Result<PublicSetLotWatchResponse, CommandError> {
    #[cfg(test)]
    let fixture = crate::surface::PageReadFixture::<
        (String, String, String, PublicSetLotWatchRequest),
        (u16, String),
    >::response((
        user_id.to_owned(),
        auction_id.to_owned(),
        lot_id.to_owned(),
        command.clone(),
    ))
    .map(|(status, body)| {
        let response = super::clock::HttpResponseFixture { status, etag: None, body: Some(body) };
        Box::pin(std::future::ready(Ok(response))) as super::clock::HttpFixtureFuture
    });
    let observation = SessionIdentityObservation::start();
    let owner_observation = SessionIdentityObservation::user_owner();
    let response = send_response(
        |abort_signal| {
            Request::put(&format!("/api/v1/lots/{lot_id}/watch"))
                .abort_signal(abort_signal)
                .header(auction_contracts::PUBLIC_EXPECTED_USER_ID_HEADER, user_id)
                .json(command)
                .map_err(|_| TransportError::Network(None))
        },
        #[cfg(test)]
        fixture,
    )
    .await
    .map_err(|error| failure(error.code(), 0))?;
    let status = response.status();
    let result: PublicSetLotWatchResponse =
        decode_account_identity_response(response, Some(user_id), &owner_observation, |value| {
            exact_fields(value, &["user_id", "auction_id", "watch"])
                && value.get("watch").is_some_and(valid_watch_value)
        })
        .await?;
    if result.user_id != user_id
        || result.auction_id != auction_id
        || result.watch.lot_id != lot_id
        || !valid_watch(&result.watch)
        || result.watch.watched != command.watched
        || result.watch.channel != command.channel
    {
        return Err(failure("invalid_response", status));
    }
    if !observation.publish(SessionIdentity::UserId(result.user_id.clone())) {
        request_public_session_check(SessionRefreshReason::CookieChanged);
        return Err(failure("session_changed", status));
    }
    Ok(result)
}

/// 缓存与 HTTP 共用校验；重复 lot、跨身份、超限和无效墓碑都失败关闭。
pub(crate) fn valid_watch_collection(
    page: &PublicLotWatchesResponse,
    user_id: &str,
    auction_id: &str,
) -> bool {
    let mut lots = BTreeSet::new();
    page.user_id == user_id
        && page.auction_id == auction_id
        && valid_uuid(user_id)
        && valid_uuid(auction_id)
        && page.watches.len() <= 10_000
        && page.watches.iter().all(|watch| valid_watch(watch) && lots.insert(&watch.lot_id))
}

fn valid_watch(watch: &PublicLotWatchResponse) -> bool {
    valid_uuid(&watch.lot_id) && watch.channel == "mm" && watch.version > 0
}

fn valid_watch_value(value: &Value) -> bool {
    exact_fields(value, &["lot_id", "watched", "channel", "version"])
        && value.get("watched").is_some_and(Value::is_boolean)
        // watch 的版本契约是完整正 i64；原 JSON 整数直接解析，不经过 JS 浮点。
        && value.get("version").and_then(Value::as_i64).is_some_and(|version| version > 0)
}

fn failure(code: &str, status: u16) -> CommandError {
    CommandError { code: code.to_owned(), status }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use auction_contracts::{
        ApiResponse, CurrentAuctionResponse, PublicLotWatchesResponse, PublicSetLotWatchRequest,
    };
    use auction_web_shared::{PageVisibility, use_second_ticker};
    use dioxus::core::{ElementId, Mutation};
    use dioxus::html::PlatformEventData;
    use dioxus::prelude::*;
    use serde_json::{Value, json};

    use super::{exact_fields, valid_watch_collection, valid_watch_value};
    use crate::api::session::decode_identity_response;
    use crate::api::{CommandError, LotEnvelope, PublicSession};
    use crate::surface::{
        AuctionIndex, CurrentAuctionResource, LotViewRequest, PageBrowserFixture, PageReadFixture,
        PublicCommandOwner, PublicDisplayedAuction, PublicPageRealtimeTarget,
        PublicReadinessResource, PublicSessionResource, page_test_contexts, rendered_event_target,
        run_page_events, settle_page,
    };

    const WATCH_CONFLICT: &str =
        include_str!("../../../auction-contracts/tests/fixtures/public/error-watch-conflict.json");
    const WATCHES: &str =
        include_str!("../../../auction-contracts/tests/fixtures/public/lot-watches.json");
    const AUCTION: &str = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
    const LOT: &str = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2c";

    thread_local! {
        static WATCH_SESSION: Cell<Option<(PublicSessionResource, Signal<bool>)>> =
            const { Cell::new(None) };
    }

    #[test]
    fn lot_watches_decode_backend_contract_fixtures() {
        for (body, watched) in [
            (WATCHES, true),
            (
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/lot-watches-removed.json"
                ),
                false,
            ),
        ] {
            let page: PublicLotWatchesResponse =
                decode_identity_response(200, Some(body), |value| {
                    exact_fields(value, &["user_id", "auction_id", "watches"])
                        && value
                            .get("watches")
                            .and_then(Value::as_array)
                            .is_some_and(|rows| rows.iter().all(valid_watch_value))
                })
                .expect("装备标记");
            assert!(valid_watch_collection(&page, AUCTION, AUCTION));
            assert_eq!(page.watches[0].watched, watched);
        }
    }

    #[test]
    fn malformed_watch_conflicts_leave_the_command_outcome_uncertain() {
        let error = decode_identity_response::<()>(409, Some(WATCH_CONFLICT), Value::is_null)
            .expect_err("生产冲突信封");
        assert_eq!(error.into_read_code(), "watch_conflict");

        let mut envelope: Value = serde_json::from_str(WATCH_CONFLICT).expect("标记冲突信封");
        let positional = json!([envelope["code"], envelope["message"], envelope["data"]]);
        let bad_extension = WATCH_CONFLICT.replace("\"request_id\": null", "\"request_id\": 1e999");
        let invalid_data = [json!(true), json!([])].map(|data| {
            envelope["data"] = data;
            envelope.to_string()
        });
        for body in invalid_data.into_iter().chain([positional.to_string(), bad_extension]) {
            let error = decode_identity_response::<()>(409, Some(&body), Value::is_null)
                .expect_err("损坏信封");
            let error = CommandError { code: error.into_read_code(), status: 409 };
            assert_eq!(error.code, "invalid_response");
            assert!(error.outcome_uncertain());
        }
    }

    #[derive(Clone, PartialEq, Routable)]
    enum WatchTestRoute {
        #[route("/")]
        AuctionIndex {},
    }

    fn watch_app() -> Element {
        page_test_contexts();
        use_second_ticker();
        use_context_provider(PageVisibility::new);
        use_context_provider(|| Signal::new("main".to_owned()));
        use_context_provider(|| PublicCommandOwner(Signal::new(Some(AUCTION.to_owned()))));
        use_context_provider(|| PublicDisplayedAuction(Signal::new(None)));
        use_context_provider(|| PublicPageRealtimeTarget(Signal::new(None)));
        use_context_provider(|| LotViewRequest(Signal::new(None)));
        let ready: PublicReadinessResource = use_resource(|| async { Ok(true) });
        use_context_provider(|| ready);
        let hold_session = use_signal(|| false);
        let session: PublicSessionResource = use_resource(move || async move {
            if hold_session() {
                std::future::pending::<()>().await;
            }
            let response: ApiResponse<Option<PublicSession>> = serde_json::from_str(include_str!(
                "../../../auction-contracts/tests/fixtures/public/session-permanent.json"
            ))
            .expect("当前会话样本");
            Ok(response.data)
        });
        WATCH_SESSION.set(Some((session, hold_session)));
        use_context_provider(|| session);
        let current: CurrentAuctionResource = use_resource(|| async {
            let response: ApiResponse<CurrentAuctionResponse> = serde_json::from_str(include_str!(
                "../../../auction-contracts/tests/fixtures/public/auction-current-proxy-mode.json"
            ))
            .expect("当前拍卖样本");
            ("main".to_owned(), Ok(response.data.auction))
        });
        use_context_provider(|| current);
        rsx! { Router::<WatchTestRoute> {} }
    }

    fn click_watch_control(dom: &VirtualDom, target: ElementId) {
        // 直接触发控件的监听器，隔离标记与表格行打开详情的交互。
        let data = PlatformEventData::new(Box::new(String::new()));
        dom.runtime().handle_event("click", Event::new(Rc::new(data), false).into_any(), target);
    }

    #[test]
    fn malformed_watch_conflict_retries_the_same_command() {
        run_page_events(
            module_path!(),
            "malformed_watch_conflict_retries_the_same_command",
            || {
                let requests = Rc::new(RefCell::new(Vec::new()));
                let writes = PageReadFixture::new({
                    let requests = Rc::clone(&requests);
                    move |(user, auction, lot, command): (
                        String,
                        String,
                        String,
                        PublicSetLotWatchRequest,
                    )| {
                        assert_eq!(
                            (user.as_str(), auction.as_str(), lot.as_str()),
                            (AUCTION, AUCTION, LOT)
                        );
                        let mut envelope: Value =
                            serde_json::from_str(WATCH_CONFLICT).expect("标记冲突样本");
                        let mut requests = requests.borrow_mut();
                        envelope["data"] = match requests.len() {
                            0 => json!(true),
                            1 => json!([]),
                            2 => Value::Null,
                            _ => panic!("合法冲突后不得继续重试"),
                        };
                        requests.push(serde_json::to_value(command).expect("标记请求"));
                        (409_u16, envelope.to_string())
                    }
                });
                let mut dom = VirtualDom::new(watch_app)
                    .with_root_context(writes)
                    .with_root_context(PageBrowserFixture::new("/", ""))
                    .with_root_context(PageReadFixture::new(|_: String| {
                        let response: ApiResponse<LotEnvelope> =
                            serde_json::from_str(include_str!(
                                "../../../auction-contracts/tests/fixtures/public/lots.json"
                            ))
                            .expect("拍品目录样本");
                        Ok::<_, String>(response.data)
                    }))
                    .with_root_context(PageReadFixture::new(
                        |(user, auction): (String, String)| {
                            assert_eq!((user.as_str(), auction.as_str()), (AUCTION, AUCTION));
                            let response: ApiResponse<PublicLotWatchesResponse> =
                                serde_json::from_str(WATCHES).expect("标记列表样本");
                            Ok::<_, CommandError>(response.data)
                        },
                    ));
                dom.rebuild_in_place();
                settle_page(&mut dom);
                let toggle = rendered_event_target(
                    &dom,
                    "button",
                    "class",
                    "public-lot-watch__toggle",
                    "onclick",
                );
                click_watch_control(&dom, toggle);
                let uncertain = settle_page(&mut dom);
                let retry = uncertain
                    .iter()
                    .find_map(|mutation| match mutation {
                        Mutation::NewEventListener { name, id } if name == "click" => Some(*id),
                        _ => None,
                    })
                    .expect("损坏冲突响应后仍可原样重试");
                assert_eq!(requests.borrow().len(), 1);
                let (_, mut hold) = WATCH_SESSION.get().unwrap();
                hold.set(true);
                settle_page(&mut dom);
                assert!(WATCH_SESSION.get().unwrap().0.pending());
                click_watch_control(&dom, retry);
                settle_page(&mut dom);
                assert_eq!(requests.borrow().len(), 1, "复核期间重试不得发送");
                hold.set(false);
                settle_page(&mut dom);
                assert_eq!(requests.borrow().len(), 1, "恢复后不得自动重放");
                click_watch_control(&dom, retry);
                settle_page(&mut dom);
                assert_eq!(requests.borrow().len(), 2);
                click_watch_control(&dom, retry);
                settle_page(&mut dom);
                let requests = requests.borrow();
                assert_eq!(requests.len(), 3);
                assert_eq!(requests[0]["expected_version"], 1);
                assert_eq!(requests[0]["watched"], false);
                assert_eq!(requests[1], requests[0]);
                assert_eq!(requests[2], requests[0]);
            },
        );
    }
}
