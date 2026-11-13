//! 进程构建号：HTTP 响应头与 WebSocket 首帧使用同一份值。
//!
//! 发布构建通过 `AUCTION_BUILD_ID` 注入；浏览器构建号由 `auction-web-shared` 用同一表达式
//! 展开。两者只用于发现部署后仍打开的旧页面，不参与鉴权、幂等或缓存决定。契约见
//! `docs/contracts/routes.md` 与 `docs/contracts/realtime.md`。

use std::sync::LazyLock;

/// 每个 HTTP 响应都携带的构建号响应头；浏览器用它比对自身构建号。
pub(crate) const BUILD_ID_HEADER: &str = "x-auction-build";

/// 与 `auction-web-shared::build_id` 使用同一表达式；未注入时退回本 crate 的包版本。
const INJECTED_BUILD_ID: &str = match option_env!("AUCTION_BUILD_ID") {
    Some(value) => value,
    None => env!("CARGO_PKG_VERSION"),
};

/// 构建号只允许 1–64 个 URL 与 HTTP 头安全字符，与浏览器端的校验保持同一闭集。
fn is_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// 非法注入回退包版本：错误的构建号不能让响应头构造失败，也不能把任意文本写进实时首帧。
static BUILD_ID: LazyLock<&'static str> = LazyLock::new(|| {
    if is_valid(INJECTED_BUILD_ID) { INJECTED_BUILD_ID } else { env!("CARGO_PKG_VERSION") }
});

pub(crate) fn build_id() -> &'static str {
    *BUILD_ID
}

/// WebSocket 建连后的第一个服务端文本帧；浏览器据此在部署后提示刷新。
pub(crate) fn build_hello_frame() -> String {
    format!(r#"{{"type":"hello","build":"{}"}}"#, build_id())
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::server::build_identity as parent;

    #[test]
    fn build_id_is_always_a_safe_header_value() {
        let value = build_id();
        assert!(is_valid(value), "{value}");
        assert!(!value.contains('"'), "{value}");
    }

    #[test]
    fn hello_frame_carries_exactly_the_build_id() {
        assert_eq!(build_hello_frame(), format!(r#"{{"type":"hello","build":"{0}"}}"#, build_id()));
    }

    #[test]
    fn validation_matches_the_browser_closed_set() {
        assert!(is_valid("0.1.0"));
        assert!(is_valid("build_2026-11-04.abc"));
        assert!(!is_valid(""));
        assert!(!is_valid("two words"));
        assert!(!is_valid("a/b"));
        assert!(!is_valid(&"a".repeat(65)));
    }
}
