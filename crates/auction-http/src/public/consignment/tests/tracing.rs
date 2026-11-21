//! 验证真实 HTTP 边界的事件和 span 均不记录装备凭证。

use std::fmt;
use std::sync::{Arc, Mutex};

use auction_application::consignment::ConsignmentError;
use axum::body::Body;
use axum::http::{HeaderValue, Method, header};
use tracing::field::{Field, Visit};
use tracing::level_filters::LevelFilter;
use tracing::span::{Attributes, Id, Record};
use tracing::subscriber::{Interest, NoSubscriber};
use tracing::{Dispatch, Event, Metadata, Subscriber};

use crate::public::consignment::tests::fixtures::{
    Backend, FAKE_KEY, FAKE_LINK, GOOD_ORIGIN, LISTINGS, SELLER, WriteOutcome, get, login, request,
    router, send, setup,
};

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<String>>>);

impl Visit for Capture {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.0.lock().expect("日志记录").push(format!("{}={value:?}", field.name()));
    }
}

impl Subscriber for Capture {
    fn register_callsite(&self, _metadata: &'static Metadata<'static>) -> Interest {
        Interest::always()
    }
    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(LevelFilter::TRACE)
    }
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, attributes: &Attributes<'_>) -> Id {
        attributes.record(&mut self.clone());
        Id::from_u64(1)
    }
    fn record(&self, _span: &Id, values: &Record<'_>) {
        values.record(&mut self.clone());
    }
    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}
    fn event(&self, event: &Event<'_>) {
        event.record(&mut self.clone());
    }
    fn enter(&self, _span: &Id) {}
    fn exit(&self, _span: &Id) {}
}

#[tokio::test]
async fn equipment_credential_is_absent_from_response_and_http_tracing() {
    let capture = Capture::default();
    let backend = Arc::new(Backend::default());
    let app = router(setup(true, &backend));
    let register = |command_id: &str| {
        let raw = serde_json::json!({
            "command_id": command_id,
            "mode": "consigned",
            "equipment_url": FAKE_LINK,
            "price": 101,
            "expected_fee_bps": 500,
        });
        let mut req = request(Method::POST, LISTINGS, Body::from(raw.to_string()));
        req.headers_mut().insert(header::ORIGIN, HeaderValue::from_static(GOOD_ORIGIN));
        req.headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        login(&mut req, SELLER);
        req
    };
    let good_uuid = "00000000-0000-4000-8000-000000000065";
    // 两个活动分发器让首次在并行测试线程命中的调用点按注册表计算兴趣，
    // 避免无订阅者的线程把调用点缓存为所有线程都忽略。
    let _other_dispatch = Dispatch::new(NoSubscriber::default());
    let _subscriber = tracing::subscriber::set_default(capture.clone());
    send(&app, register(good_uuid)).await.assert(202, "accepted");
    send(&app, register(FAKE_LINK)).await.assert(400, "invalid_command_id");
    *backend.command_outcome.lock().expect("命令结果") =
        WriteOutcome::Err(ConsignmentError::OutcomeUnknown);
    send(&app, register(good_uuid)).await.assert(503, "outcome_unknown");

    let encoded: String = url::form_urlencoded::byte_serialize(FAKE_LINK.as_bytes()).collect();
    send(&app, get(&format!("{LISTINGS}?unknown={encoded}"))).await.assert(400, "invalid_query");
    send(&app, get(&format!("{LISTINGS}/{FAKE_KEY}"))).await.assert(400, "invalid_listing_id");

    let logs = capture.0.lock().expect("HTTP 日志");
    assert!(
        logs.iter().any(|field| field.starts_with("request_id=")),
        "缺请求 ID 字段，已见字段名 {:?}",
        logs.iter()
            .filter_map(|field| field.split_once('=').map(|(name, _)| name))
            .collect::<Vec<_>>()
    );
    assert!(logs.iter().any(|field| field.starts_with("route_pattern=")));
    assert!(logs.iter().any(|field| field == "status=503"));
    assert!(
        !logs.iter().any(|field| field.contains(FAKE_LINK) || field.contains(FAKE_KEY)),
        "日志含读取凭证"
    );
}
