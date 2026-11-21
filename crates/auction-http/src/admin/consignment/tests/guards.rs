//! 三条路由的认证、写守卫顺序及拒绝时的零业务调用。

use axum::body::Body;
use axum::http::{HeaderValue, Method, StatusCode, header};

use super::fixture::headers;
use super::fixture::{
    Fixture, LIST_PATH, REASON, SessionCall, body, delist_path, detail_path, failure, response,
    send,
};
use crate::admin::ADMIN_CSRF_HEADER;

#[tokio::test]
async fn all_routes_reject_missing_and_expired_session_before_business() {
    for enabled in [true, false] {
        let fixture = Fixture::new(enabled);
        for cookie in [
            "auction_admin_csrf=csrf-placeholder",
            "auction_admin_session=expired-placeholder; auction_admin_csrf=csrf-placeholder",
        ] {
            for (method, path, payload_ok) in [
                (Method::GET, LIST_PATH.to_owned(), true),
                (Method::GET, detail_path(), true),
                (Method::POST, delist_path(), true),
                (Method::GET, format!("{LIST_PATH}?not-a-filter=1"), false),
                (Method::GET, format!("{LIST_PATH}/bad-id"), false),
                (Method::POST, delist_path(), false),
            ] {
                let mut request_headers = headers();
                request_headers.insert(header::COOKIE, HeaderValue::from_static(cookie));
                let request_body = if payload_ok && method == Method::POST {
                    Body::from(body(REASON).to_string())
                } else {
                    request_headers.remove(header::CONTENT_TYPE);
                    Body::empty()
                };
                let reply =
                    send(fixture.router(), method, &path, request_headers, request_body).await;
                let error = failure(reply, StatusCode::UNAUTHORIZED, "unauthenticated").await;
                assert!(!error.to_string().contains(REASON));
            }
        }
        assert_eq!(
            fixture.sessions.0.lock().unwrap().as_slice(),
            &[
                SessionCall::Read,
                SessionCall::Read,
                SessionCall::Touch,
                SessionCall::Read,
                SessionCall::Read,
                SessionCall::Touch,
            ],
        );
        fixture.assert_no_business_calls();
    }
}

#[tokio::test]
async fn reads_need_session_cookies_without_requiring_csrf_header_or_renewing() {
    let fixture = Fixture::new(true);
    for path in [LIST_PATH, &detail_path()] {
        let mut request_headers = headers();
        request_headers.remove(ADMIN_CSRF_HEADER);
        let reply = send(fixture.router(), Method::GET, path, request_headers, Body::empty()).await;
        response(reply, StatusCode::OK).await;
    }
    assert_eq!(
        fixture.sessions.0.lock().unwrap().as_slice(),
        &[SessionCall::Read, SessionCall::Read]
    );
    assert_eq!(fixture.read_port().calls.lock().unwrap().len(), 2);
    assert!(fixture.command.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn delist_origin_then_csrf_then_session_precede_path_query_and_body() {
    for enabled in [true, false] {
        let fixture = Fixture::new(enabled);
        let path = format!("{LIST_PATH}/bad-id/delist?unknown=1");
        let mut bad_origin = headers();
        bad_origin
            .insert(header::ORIGIN, HeaderValue::from_static("https://other.example.invalid"));
        bad_origin.remove(ADMIN_CSRF_HEADER);
        bad_origin.remove(header::COOKIE);
        bad_origin.remove(header::CONTENT_TYPE);
        let reply = send(fixture.router(), Method::POST, &path, bad_origin, Body::empty()).await;
        failure(reply, StatusCode::FORBIDDEN, "origin_forbidden").await;

        for csrf in [None, Some("other-placeholder")] {
            let mut invalid_csrf = headers();
            invalid_csrf.insert(
                header::COOKIE,
                HeaderValue::from_static("auction_admin_csrf=csrf-placeholder"),
            );
            invalid_csrf.remove(header::CONTENT_TYPE);
            if let Some(value) = csrf {
                invalid_csrf.insert(ADMIN_CSRF_HEADER, HeaderValue::from_static(value));
            } else {
                invalid_csrf.remove(ADMIN_CSRF_HEADER);
            }
            let reply =
                send(fixture.router(), Method::POST, &path, invalid_csrf, Body::empty()).await;
            failure(reply, StatusCode::FORBIDDEN, "csrf_invalid").await;
        }
        assert!(fixture.sessions.0.lock().unwrap().is_empty());
        fixture.assert_no_business_calls();
    }
}

#[tokio::test]
async fn delist_rate_limit_wins_over_untrusted_origin_and_other_invalid_input() {
    let fixture = Fixture::with_limit(false, 1);
    let mut request_headers = headers();
    request_headers
        .insert(header::ORIGIN, HeaderValue::from_static("https://other.example.invalid"));
    request_headers.remove(ADMIN_CSRF_HEADER);
    request_headers.remove(header::COOKIE);
    let path = delist_path();
    let first =
        send(fixture.router(), Method::POST, &path, request_headers.clone(), Body::empty()).await;
    failure(first, StatusCode::FORBIDDEN, "origin_forbidden").await;
    let second = send(
        fixture.router(),
        Method::POST,
        &path,
        request_headers,
        Body::from(body(REASON).to_string()),
    )
    .await;
    assert!(second.headers().get(header::RETRY_AFTER).is_some());
    failure(second, StatusCode::TOO_MANY_REQUESTS, "rate_limited").await;
    assert!(fixture.sessions.0.lock().unwrap().is_empty());
    fixture.assert_no_business_calls();
}

#[tokio::test]
async fn unsupported_methods_keep_no_store_and_do_not_invoke_business() {
    let fixture = Fixture::new(true);
    for (method, path, allow) in [
        (Method::POST, LIST_PATH.to_owned(), "GET"),
        (Method::PATCH, detail_path(), "GET"),
        (Method::GET, delist_path(), "POST"),
    ] {
        let reply = send(fixture.router(), method, &path, headers(), Body::empty()).await;
        assert_eq!(reply.headers().get(header::ALLOW).unwrap(), allow);
        failure(reply, StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed").await;
    }
    fixture.assert_no_business_calls();
}
