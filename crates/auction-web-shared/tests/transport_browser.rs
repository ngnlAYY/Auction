//! 经真实 Fetch Response 验证共享传输层的响应头、正文和错误接线。
#![cfg(target_arch = "wasm32")]
#![allow(clippy::expect_used)]

use std::cell::Cell;
use std::mem::DropGuard;
use std::rc::Rc;

use auction_web_shared::{TransportError, send_get_observed};
use gloo_net::http::Response;
use js_sys::{Object, Promise, Reflect};
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_test::wasm_bindgen_test;
use wasm_bindgen_test::wasm_bindgen_test_configure;
use web_sys::{ReadableStream, ResponseInit};

wasm_bindgen_test_configure!(run_in_browser);

thread_local! {
    // wasm trap 可能跳过守卫，后续用例仍须恢复第一次替换前的 fetch。
    static ORIGINAL_FETCH: JsValue =
        Reflect::get(&js_sys::global(), &JsValue::from_str("fetch")).expect("读取浏览器 fetch");
}

fn response_init(status: u16) -> ResponseInit {
    let init = ResponseInit::new();
    init.set_status(status);
    init
}

async fn send(
    reply: Result<web_sys::Response, JsValue>,
) -> (Result<Response, TransportError>, Vec<(u16, bool)>) {
    let original_response = reply.as_ref().ok().cloned();
    let fetch_calls = Rc::new(Cell::new(0));
    let calls = Rc::clone(&fetch_calls);
    let replacement = Closure::<dyn FnMut() -> Promise>::new(move || {
        calls.set(calls.get() + 1);
        match &reply {
            Ok(response) => Promise::resolve(AsRef::<JsValue>::as_ref(response)),
            Err(error) => Promise::reject(error),
        }
    });
    let global = js_sys::global();
    let key = JsValue::from_str("fetch");
    let original = ORIGINAL_FETCH.with(JsValue::clone);
    assert!(
        Reflect::set(&global, &key, replacement.as_ref()).expect("替换浏览器 fetch"),
        "必须能安装 fetch 替身",
    );
    let guard = DropGuard::new(replacement, move |_| {
        assert!(
            Reflect::set(&global, &key, &original).expect("恢复浏览器 fetch"),
            "必须能恢复原 fetch",
        );
    });

    let mut observations = Vec::new();
    let result = send_get_observed("/browser-test/transport", |response| {
        // 检查原始 Response，避免晚到的回调收到重建后的未读正文而误通过。
        let body_used = original_response.as_ref().is_some_and(web_sys::Response::body_used);
        observations.push((response.status(), body_used));
    })
    .await;
    // wasm panic 可能不展开栈，先恢复 fetch，再由用例断言结果。
    drop(guard);
    assert_eq!(fetch_calls.get(), 1, "共享传输入口必须调用一次 fetch 替身");
    (result, observations)
}

async fn assert_error_status_before_body(status: u16) {
    let reply = web_sys::Response::new_with_opt_str_and_init(Some("正文"), &response_init(status))
        .expect("构造带正文的错误响应");
    let (result, observations) = send(Ok(reply)).await;
    assert_eq!(
        observations,
        [(status, false)],
        "HTTP {status} 必须恰好观察一次，且原始正文尚未读取",
    );
    let response = result.expect("HTTP 错误响应也应完成传输");
    assert_eq!(response.status(), status, "缓冲后必须保留 HTTP {status}");
    assert_eq!(response.text().await.expect("读取缓冲正文"), "正文", "HTTP {status} 正文");
}

#[wasm_bindgen_test]
async fn conflict_is_observed_before_body() {
    assert_error_status_before_body(409).await;
}

#[wasm_bindgen_test]
async fn service_unavailable_is_observed_before_body() {
    assert_error_status_before_body(503).await;
}

async fn assert_null_body(status: u16) {
    let init = response_init(status);
    let headers = web_sys::Headers::new().expect("创建响应头");
    headers.set("ETag", "\"test-tag\"").expect("写入 ETag");
    init.set_headers(&headers);
    let reply =
        web_sys::Response::new_with_opt_str_and_init(None, &init).expect("构造 null body 响应");
    let (result, _) = send(Ok(reply)).await;
    let response = result.expect("null body 响应必须能完成传输");
    assert_eq!(response.status(), status, "null body 响应的状态码");
    assert_eq!(
        response.headers().get("ETag").as_deref(),
        Some("\"test-tag\""),
        "HTTP {status} 响应的 ETag"
    );
    assert!(response.body().is_none(), "HTTP {status} 的正文必须为 null");
}

#[wasm_bindgen_test]
async fn not_modified_preserves_null_body_and_headers() {
    assert_null_body(304).await;
}

#[wasm_bindgen_test]
async fn no_content_preserves_null_body_and_headers() {
    assert_null_body(204).await;
}

#[wasm_bindgen_test]
async fn body_failure_keeps_observed_status() {
    let pull = Closure::once(|| Promise::reject(&js_sys::Error::new("正文读取失败")));
    let source = Object::new();
    // 字节流默认不预取，只有开始读取正文时才触发拒绝。
    assert!(
        Reflect::set(&source, &JsValue::from_str("type"), &JsValue::from_str("bytes"))
            .expect("选择字节流"),
    );
    assert!(Reflect::set(&source, &JsValue::from_str("pull"), pull.as_ref()).expect("注册正文流"));
    let stream = ReadableStream::new_with_underlying_source(&source).expect("构造拒绝读取的正文流");
    let reply = web_sys::Response::new_with_opt_readable_stream_and_init(
        Some(&stream),
        &response_init(410),
    )
    .expect("构造带失败正文的 HTTP 410 响应");
    let (result, observations) = send(Ok(reply)).await;
    assert_eq!(observations, [(410, false)], "正文失败前必须恰好观察一次 HTTP 410");
    assert_eq!(result.err(), Some(TransportError::Network(Some(410))), "正文失败须携带已知状态");
}

#[wasm_bindgen_test]
async fn fetch_rejection_has_no_response_head() {
    let (result, observations) = send(Err(js_sys::Error::new("请求被拒绝").into())).await;
    assert_eq!(observations, [], "fetch 拒绝时没有响应头可观察");
    assert_eq!(result.err(), Some(TransportError::Network(None)), "fetch 拒绝无 HTTP 状态");
}
