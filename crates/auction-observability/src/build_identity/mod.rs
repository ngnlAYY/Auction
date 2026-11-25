//! 进程构建号的唯一 owner：HTTP 响应头、实时首帧与寄售启用回执核对都取这里的值。
//!
//! 发布构建通过 `AUCTION_BUILD_ID` 注入；未注入或非法时退回包版本。浏览器构建号由
//! `auction-web-shared` 在 wasm 侧用同一规则展开。构建号只用于发现旧页面与绑定部署回执，
//! 不参与鉴权、幂等或缓存决定。

use std::sync::LazyLock;

const INJECTED_BUILD_ID: &str = match option_env!("AUCTION_BUILD_ID") {
    Some(value) => value,
    None => env!("CARGO_PKG_VERSION"),
};

/// 非法注入回退包版本：错误的构建号不能让响应头构造失败，也不能把任意文本写进首帧或回执比对。
static BUILD_ID: LazyLock<&'static str> = LazyLock::new(|| {
    if is_valid_build_id(INJECTED_BUILD_ID) { INJECTED_BUILD_ID } else { env!("CARGO_PKG_VERSION") }
});

/// 本进程的构建号。
pub fn process_build_id() -> &'static str {
    *BUILD_ID
}

/// 构建号只允许 1–64 个 URL 与 HTTP 头安全字符，与浏览器端的校验保持同一闭集；
/// 首字符必须是字母数字，使 `.`、`..` 不能成为构建目录名。
pub fn is_valid_build_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}
