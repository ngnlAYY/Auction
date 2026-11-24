//! 捕获真实 HTTP 边界的 tracing，验证运营说明不会进入诊断。

use std::fmt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use auction_application::consignment::{
    ConsignmentError, ListingClosed, ListingCommandOutcome, ListingDelistReason,
};
use auction_domain::consignment::ListingVersion;
use axum::body::Body;
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware;
use tracing::field::{Field, Visit};
use tracing::instrument::WithSubscriber as _;
use tracing::span::{Attributes, Id, Record};
use tracing::{Dispatch, Event, Metadata, Subscriber};

use super::fixture::{
    Fixture, LISTING, REASON, body, delist_path, detail_path, failure, headers, response, send,
};
use crate::middleware::boundary::{HttpBoundary, enforce};

#[derive(Default)]
struct Capture {
    fields: Mutex<String>,
    next_span: AtomicU64,
}

impl Capture {
    fn record(&self, visit: impl FnOnce(&mut FieldRecorder<'_>)) {
        visit(&mut FieldRecorder(&mut self.fields.lock().unwrap()));
    }
}

struct FieldRecorder<'a>(&'a mut String);

impl Visit for FieldRecorder<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        use std::fmt::Write as _;

        writeln!(self.0, "{}={value:?}", field.name()).unwrap();
    }
}

impl Subscriber for Capture {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, attributes: &Attributes<'_>) -> Id {
        self.record(|visitor| attributes.record(visitor));
        Id::from_u64(self.next_span.fetch_add(1, Ordering::Relaxed) + 1)
    }

    fn record(&self, _span: &Id, values: &Record<'_>) {
        self.record(|visitor| values.record(visitor));
    }

    fn record_follows_from(&self, _span: &Id, _preceding: &Id) {}

    fn event(&self, event: &Event<'_>) {
        self.record(|visitor| event.record(visitor));
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

#[tokio::test]
async fn actual_http_traces_and_write_bodies_do_not_contain_private_reasons() {
    let mut fixture = Fixture::new(false);
    fixture.read_port_mut().detail.as_mut().unwrap().as_mut().unwrap().record.admin_delist_reason =
        Some(ListingDelistReason::new(REASON.to_owned()).unwrap());
    fixture.command.results.lock().unwrap().extend([
        Err(ConsignmentError::OutcomeUnknown),
        Ok(ListingCommandOutcome::Accepted(
            ListingClosed::new(LISTING, ListingVersion::new(8).unwrap(), None).unwrap(),
        )),
    ]);
    let router =
        fixture.router().layer(middleware::from_fn_with_state(HttpBoundary::default(), enforce));
    let dispatch = Dispatch::new(Capture::default());
    let invalid_marker = "私有夹具标记";
    let request_id = "consignment-fixture-request";
    async {
        let detail =
            send(router.clone(), Method::GET, &detail_path(), headers(), Body::empty()).await;
        let detail = response(detail, StatusCode::OK).await;
        assert!(detail["data"]["record"]["admin_delist_reason"].as_str() == Some(REASON));
        // 回调点的兴趣是进程级缓存：并行用例若先在没有订阅者的线程里触发边界日志的回调点，
        // 兴趣会被缓存成 never，本用例的事件随之被丢弃。上面的请求已让回调点完成登记，
        // 这里重建缓存，后面的写请求才是断言依据。
        tracing::callsite::rebuild_interest_cache();
        for (reason, status, code) in [
            (REASON.to_owned(), StatusCode::SERVICE_UNAVAILABLE, "outcome_unknown"),
            (format!("{invalid_marker}\u{2028}第二行"), StatusCode::BAD_REQUEST, "invalid_reason"),
        ] {
            let mut request_headers = headers();
            request_headers.insert("x-request-id", HeaderValue::from_static(request_id));
            let reply = send(
                router.clone(),
                Method::POST,
                &delist_path(),
                request_headers,
                Body::from(body(&reason).to_string()),
            )
            .await;
            let error = failure(reply, status, code).await;
            assert_eq!(error["request_id"], request_id);
            let error_body = error.to_string();
            assert!(!error_body.contains(REASON));
            assert!(!error_body.contains(invalid_marker));
        }
        let reply = send(
            router,
            Method::POST,
            &delist_path(),
            headers(),
            Body::from(body(REASON).to_string()),
        )
        .await;
        let receipt = response(reply, StatusCode::OK).await;
        assert_eq!(receipt["data"]["disposition"], "closed");
        assert!(!receipt.to_string().contains(REASON));
    }
    .with_subscriber(dispatch.clone())
    .await;
    let capture = dispatch.downcast_ref::<Capture>().unwrap();
    let captured = capture.fields.lock().unwrap();
    assert!(captured.contains("route_pattern"));
    assert!(captured.contains(request_id));
    assert!(captured.contains("HTTP request completed"));
    assert!(!captured.contains(REASON));
    assert!(!captured.contains(invalid_marker));
}
