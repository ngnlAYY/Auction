//! 经 Public 拍品详情读取验证真实 Fetch 响应的条件请求与驱逐接线。

use std::cell::RefCell;
use std::future::poll_fn;
use std::mem::DropGuard;
use std::pin::pin;
use std::rc::Rc;

use dioxus::prelude::{ScopeId, VNode, VirtualDom};
use js_sys::{Object, Promise, Reflect};
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_test::wasm_bindgen_test;
use wasm_bindgen_test::wasm_bindgen_test_configure;
use web_sys::{ReadableStream, Request, Response, ResponseInit};

use super::queries::{LotDetailFetchError, fetch_lot_detail};
use super::swr_cache::invalidate_all;
use crate::api::LotEnvelope;

wasm_bindgen_test_configure!(run_in_browser);

const LOT_FIXTURE: &str = include_str!("../../../auction-contracts/tests/fixtures/public/lot.json");
const FIXTURE_LOT_ID: &str = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2c";
const ETAG: &str = "\"lot-v1\"";

thread_local! {
    // wasm trap 可能跳过守卫，后续用例仍须恢复第一次替换前的 fetch。
    static ORIGINAL_FETCH: JsValue =
        Reflect::get(&js_sys::global(), &JsValue::from_str("fetch")).expect("读取浏览器 fetch");
}

fn response(status: u16, body: Option<&str>) -> Response {
    let init = ResponseInit::new();
    init.set_status(status);
    Response::new_with_opt_str_and_init(body, &init).expect("构造拍品响应")
}

fn lot_response(status: u16, lot_id: &str) -> Response {
    let body = LOT_FIXTURE.replace(FIXTURE_LOT_ID, lot_id);
    let response = response(status, Some(&body));
    response.headers().set("ETag", ETAG).expect("写入拍品 ETag");
    response
}

async fn read_lot(
    lot_id: &str,
    reply: Response,
    expected_etag: Option<&str>,
) -> Result<LotEnvelope, LotDetailFetchError> {
    let status = reply.status();
    let requests = Rc::new(RefCell::new(Vec::new()));
    let captured = Rc::clone(&requests);
    let replacement = Closure::<dyn FnMut(Request) -> Promise>::new(move |request| {
        captured.borrow_mut().push(request);
        Promise::resolve(AsRef::<JsValue>::as_ref(&reply))
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

    let dom = VirtualDom::new(VNode::empty);
    let mut read = pin!(fetch_lot_detail(lot_id.to_owned()));
    // 每次轮询都进入同一个根作用域，拍品夹具查找返回 None 后继续走浏览器传输。
    let result = poll_fn(|cx| dom.in_scope(ScopeId::ROOT, || read.as_mut().poll(cx))).await;
    // wasm panic 可能不展开栈，先恢复 fetch，再检查请求与结果。
    drop(guard);
    let requests = requests.borrow();
    assert_eq!(requests.len(), 1, "拍品 {lot_id} 的 HTTP {status} 夹具应收到一次请求");
    let request = &requests[0];
    let path = format!("/api/v1/lots/{lot_id}");
    assert!(request.url().ends_with(&path), "预期请求 {path}，实际是 {}", request.url());
    assert_eq!(request.method(), "GET", "拍品 {lot_id} 应使用 GET");
    let received_etag = request.headers().get("If-None-Match").expect("读取条件请求头");
    assert_eq!(
        received_etag.as_deref(),
        expected_etag,
        "拍品 {lot_id} 的 HTTP {status} 夹具收到的 If-None-Match",
    );
    result
}

#[wasm_bindgen_test]
async fn not_modified_reuses_lot_and_keeps_its_etag() {
    let lot_id = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1b01";
    let cached = read_lot(lot_id, lot_response(200, lot_id), None).await.expect("200 应写入拍品");

    // 用生产失效入口强制再验证，保留旧正文和 ETag，不等待 TTL。
    invalidate_all();
    let revalidated =
        read_lot(lot_id, response(304, None), Some(ETAG)).await.expect("304 应复用拍品正文");
    assert_eq!(revalidated, cached, "304 应返回原拍品");

    invalidate_all();
    assert_eq!(
        read_lot(lot_id, response(304, None), Some(ETAG)).await,
        Ok(cached),
        "304 后下一次再验证仍应携带旧 ETag 并复用原拍品",
    );
}

async fn assert_missing_evicts_lot(lot_id: &str, missing: Response) {
    let status = missing.status();
    read_lot(lot_id, lot_response(200, lot_id), None).await.expect("200 应写入拍品");
    invalidate_all();
    assert_eq!(
        read_lot(lot_id, missing, Some(ETAG)).await,
        Err(LotDetailFetchError::Missing),
        "HTTP {status} 的响应头已经确认拍品不存在",
    );
    // 先前的失效已阻止新鲜命中。如果驱逐漏接，这次仍会发送旧 ETag。
    read_lot(lot_id, lot_response(200, lot_id), None)
        .await
        .expect("驱逐后的全量响应应能重新读取拍品");
}

#[wasm_bindgen_test]
async fn not_found_evicts_lot_before_the_next_get() {
    assert_missing_evicts_lot("0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1b02", response(404, Some("{}")))
        .await;
}

#[wasm_bindgen_test]
async fn gone_evicts_lot_even_if_body_read_fails() {
    let pull = Closure::once(|| Promise::reject(&js_sys::Error::new("正文读取失败")));
    let source = Object::new();
    // 字节流默认不预取，让驱逐发生在正文读取触发拒绝之前。
    assert!(
        Reflect::set(&source, &JsValue::from_str("type"), &JsValue::from_str("bytes"))
            .expect("选择字节流"),
    );
    assert!(Reflect::set(&source, &JsValue::from_str("pull"), pull.as_ref()).expect("注册正文流"));
    let stream = ReadableStream::new_with_underlying_source(&source).expect("构造拒绝读取的正文流");
    let init = ResponseInit::new();
    init.set_status(410);
    let reply = Response::new_with_opt_readable_stream_and_init(Some(&stream), &init)
        .expect("构造带失败正文的 HTTP 410 响应");
    assert_missing_evicts_lot("0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1b03", reply).await;
}

#[wasm_bindgen_test]
async fn conflict_rejects_even_a_valid_lot_body() {
    let lot_id = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1b04";
    read_lot(lot_id, lot_response(200, lot_id), None).await.expect("200 应接受同一份合法正文");
    invalidate_all();
    assert_eq!(
        read_lot(lot_id, lot_response(409, lot_id), Some(ETAG)).await,
        Err(LotDetailFetchError::Unavailable),
        "HTTP 409 即使带合法拍品正文也必须拒绝",
    );
}
