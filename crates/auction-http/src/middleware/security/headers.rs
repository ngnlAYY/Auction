//! 正式表面与首次设置共用的浏览器安全头；不接管鉴权、缓存或请求时限。

use axum::http::{HeaderMap, HeaderValue, header};

/// 浏览器脚本策略的闭集。装配根按 surface 的实际技术栈显式选择；合并服务里 Public 与
/// Admin 可以不同，因此不能从 `auction-http` 的全局 Cargo feature 推断。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebScriptPolicy {
    /// Dioxus/WASM CSR：`script-src 'self' 'unsafe-eval' 'wasm-unsafe-eval'`，因为 WASM
    /// 编译与解释器执行需要这两项放宽；其余指令与 Solid 相同。
    Dioxus,
    /// SolidJS：`script-src 'self'`；其余指令与 Dioxus 相同，保留两者的内联样式例外。
    Solid,
}

impl WebScriptPolicy {
    fn content_security_policy(self) -> &'static str {
        match self {
            Self::Dioxus => DIOXUS_CONTENT_SECURITY_POLICY,
            Self::Solid => SOLID_CONTENT_SECURITY_POLICY,
        }
    }
}

/// Dioxus 页面的 CSP。两处已知放宽是经过复核的结论，不是遗漏：
///
/// - `script-src 'unsafe-eval' 'wasm-unsafe-eval'`：Dioxus 0.7 的 CSR 启动依赖
///   `WebAssembly.instantiate` 编译权限，其解释器在执行事件处理时还会走 `new Function`。
///   去掉任一项都会让两端 WASM 在加载阶段失败，当前没有可用的等价能力开关。剩余风险
///   由同源脚本限制收窄：页面不加载第三方脚本、不含内联脚本，`eval` 只能执行同源
///   资源里已有的代码；攻击者要先取得同源脚本执行权才能利用它，而那一前提本身已是
///   更严重的缺陷。因此接受该例外，并在每次升级 Dioxus 时重新核对能否去掉。
/// - `style-src 'unsafe-inline'`：`index.html` 的首屏防闪样式与组件 `style` 属性是内联
///   样式，CSP 无法用 nonce 覆盖 Dioxus 运行时写入的属性。内联样式不能执行脚本，
///   风险限于视觉伪装，且页面不引入第三方样式源。
const DIOXUS_CONTENT_SECURITY_POLICY: &str = concat!(
    "default-src 'self'; ",
    "script-src 'self' 'unsafe-eval' 'wasm-unsafe-eval'; ",
    "style-src 'self' 'unsafe-inline'; ",
    "object-src 'none'; ",
    "base-uri 'self'; ",
    "form-action 'self'; ",
    "frame-ancestors 'none'",
);

/// SolidJS 页面的 CSP：脚本只允许同源，保留与 Dioxus 相同的内联样式例外。
const SOLID_CONTENT_SECURITY_POLICY: &str = concat!(
    "default-src 'self'; ",
    "script-src 'self'; ",
    "style-src 'self' 'unsafe-inline'; ",
    "object-src 'none'; ",
    "base-uri 'self'; ",
    "form-action 'self'; ",
    "frame-ancestors 'none'",
);

/// 只在浏览器实际经 HTTPS 访问的响应上发送。明文 HTTP 上的 HSTS 会被浏览器忽略，
/// 而开发/测试、本地回环与未经代理的明文部署必须保持可用。
///
/// 不附加 `includeSubDomains` 与 `preload`：本站可能只是共享父域的一个子域，递归强制
/// 兄弟子域 HTTPS 超出本服务的控制范围，且 preload 一旦生效无法在服务端撤销。
const HSTS_VALUE: &str = "max-age=31536000";

/// 写入统一的防嵌入、来源及浏览器能力限制；调用者仍须独立决定响应缓存策略。
///
/// `hsts` 由调用方根据请求的实际协议决定：只有确认浏览器经 HTTPS 访问时才为 true，
/// 反向代理后只能采信可信上游写入的 `X-Forwarded-Proto`。
pub fn apply_browser_security_headers(
    headers: &mut HeaderMap,
    hsts: bool,
    web_script_policy: WebScriptPolicy,
) {
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(web_script_policy.content_security_policy()),
    );
    if hsts {
        headers.insert(header::STRICT_TRANSPORT_SECURITY, HeaderValue::from_static(HSTS_VALUE));
    }
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), geolocation=(), microphone=()"),
    );
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_script_policies_write_their_exact_csp() {
        for (policy, expected) in [
            (
                WebScriptPolicy::Dioxus,
                "default-src 'self'; script-src 'self' 'unsafe-eval' 'wasm-unsafe-eval'; \
                 style-src 'self' 'unsafe-inline'; object-src 'none'; base-uri 'self'; \
                 form-action 'self'; frame-ancestors 'none'",
            ),
            (
                WebScriptPolicy::Solid,
                "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
                 object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'",
            ),
        ] {
            let mut headers = HeaderMap::new();
            apply_browser_security_headers(&mut headers, false, policy);
            assert_eq!(
                headers.get(header::CONTENT_SECURITY_POLICY).and_then(|value| value.to_str().ok()),
                Some(expected)
            );
        }
    }

    #[test]
    fn hsts_is_only_written_when_the_request_was_https() {
        let mut plain = HeaderMap::new();
        apply_browser_security_headers(&mut plain, false, WebScriptPolicy::Dioxus);
        assert!(!plain.contains_key(header::STRICT_TRANSPORT_SECURITY));

        let mut secure = HeaderMap::new();
        apply_browser_security_headers(&mut secure, true, WebScriptPolicy::Dioxus);
        assert_eq!(
            secure.get(header::STRICT_TRANSPORT_SECURITY).and_then(|value| value.to_str().ok()),
            Some("max-age=31536000")
        );
        // 不递归强制子域、不进入 preload。
        assert!(
            !secure[header::STRICT_TRANSPORT_SECURITY]
                .to_str()
                .unwrap()
                .contains("includeSubDomains")
        );
    }

    #[test]
    fn other_browser_headers_are_unchanged() {
        let mut headers = HeaderMap::new();
        apply_browser_security_headers(&mut headers, true, WebScriptPolicy::Dioxus);
        assert_eq!(headers.get("x-frame-options").unwrap(), "DENY");
        assert_eq!(headers.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(), "nosniff");
        assert_eq!(headers.get(header::REFERRER_POLICY).unwrap(), "no-referrer");
        assert_eq!(
            headers.get("permissions-policy").unwrap(),
            "camera=(), geolocation=(), microphone=()"
        );
    }
}
