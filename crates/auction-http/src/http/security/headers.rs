//! 正式表面与首次设置共用的浏览器安全头；不接管鉴权、缓存或请求时限。

use axum::http::{HeaderMap, HeaderValue, header};

/// 写入统一的防嵌入、来源及浏览器能力限制；调用者仍须独立决定响应缓存策略。
pub fn apply_browser_security_headers(headers: &mut HeaderMap) {
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        // Dioxus 的 WASM 启动需要 `wasm-unsafe-eval`，解释器还会使用动态 JavaScript；两端
        // index 中的首屏防闪样式和组件 style 属性需要 `style-src 'unsafe-inline'`。
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self' 'unsafe-eval' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; object-src 'none'; base-uri 'self'; frame-ancestors 'none'",
        ),
    );
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), geolocation=(), microphone=()"),
    );
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
}
