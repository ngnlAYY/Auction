//! 静态资源与 CDN 的缓存策略。

use std::path::Path;

use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::Response;

use crate::WebScriptPolicy;

#[derive(Clone, Copy)]
pub(super) enum CachePolicy {
    NoCache,
    Immutable,
    /// 稳定文件名的站点图标：允许浏览器与 CDN 保存一天，过期后按 ETag 再验证。
    Favicon,
}

impl CachePolicy {
    pub(super) const fn value(self) -> &'static str {
        match self {
            Self::NoCache => "no-cache",
            Self::Immutable => "public, max-age=31536000, immutable",
            Self::Favicon => "public, max-age=86400",
        }
    }
}

// 只有本模块的精确静态响应可以签发此标记；API 的 Cache-Control 不能启用 CDN 缓存。
#[derive(Clone, Copy)]
pub(super) struct StaticCachePolicy(pub(super) CachePolicy);

/// 返回静态 handler 明确批准的 CDN 策略，其余响应按 `Cache-Control` 语义映射。
pub(crate) fn cdn_cache_policy(response: &Response) -> HeaderValue {
    if let Some(policy) = response.extensions().get::<StaticCachePolicy>() {
        return HeaderValue::from_static(policy.0.value());
    }
    shared_cache_policy(response.headers()).unwrap_or_else(|| HeaderValue::from_static("no-store"))
}

/// 只有 `public` 且带有效新鲜度的动态响应获得边缘缓存资格；`no-store`/`private` 永不放宽，
/// `no-cache` 保留强制再验证，其余一律禁止边缘保存。
///
/// 新鲜度取 `s-maxage`（缺省 `max-age`）并统一写成 `max-age`：Cloudflare 在指令含 `s-maxage`
/// 时不执行 `stale-while-revalidate`/`stale-if-error`，原样复制会让这两个窗口在边缘失效。
pub(super) fn shared_cache_policy(headers: &HeaderMap) -> Option<HeaderValue> {
    let mut eligible = None;
    let mut any_malformed_freshness = false;
    let mut no_cache = false;
    for value in headers.get_all(header::CACHE_CONTROL) {
        let Ok(text) = value.to_str() else {
            return None;
        };
        let mut is_public = false;
        let mut field_malformed_freshness = false;
        let mut max_age = None;
        let mut shared_max_age = None;
        let mut stale_while_revalidate = None;
        let mut stale_if_error = None;
        for directive in text.split(',') {
            let directive = directive.trim();
            let (name, argument) = match directive.split_once('=') {
                Some((name, argument)) => (name.trim(), Some(argument.trim())),
                None => (directive, None),
            };
            if name.eq_ignore_ascii_case("no-store") || name.eq_ignore_ascii_case("private") {
                return None;
            }
            let seconds = argument.and_then(delta_seconds);
            if name.eq_ignore_ascii_case("no-cache") {
                no_cache = true;
            } else if name.eq_ignore_ascii_case("public") {
                is_public = true;
            } else if name.eq_ignore_ascii_case("max-age") {
                field_malformed_freshness |= seconds.is_none();
                max_age = seconds;
            } else if name.eq_ignore_ascii_case("s-maxage") {
                field_malformed_freshness |= seconds.is_none();
                shared_max_age = seconds;
            } else if name.eq_ignore_ascii_case("stale-while-revalidate") {
                stale_while_revalidate = seconds;
            } else if name.eq_ignore_ascii_case("stale-if-error") {
                stale_if_error = seconds;
            }
        }
        // 新鲜度写坏的响应不猜测意图，按无资格处理。
        any_malformed_freshness |= field_malformed_freshness;
        let freshness = if field_malformed_freshness { None } else { shared_max_age.or(max_age) };
        if let (true, Some(freshness), None) = (is_public, freshness, &eligible) {
            let mut policy = format!("public, max-age={freshness}");
            for (name, seconds) in [
                ("stale-while-revalidate", stale_while_revalidate),
                ("stale-if-error", stale_if_error),
            ] {
                if let Some(seconds) = seconds {
                    policy.push_str(&format!(", {name}={seconds}"));
                }
            }
            eligible = HeaderValue::from_str(&policy).ok();
        }
    }
    if no_cache {
        return Some(HeaderValue::from_static("no-cache"));
    }
    if any_malformed_freshness { None } else { eligible }
}

/// 只接受非负十进制秒数；`max-age` 历史上可能带引号，两种写法都按有效新鲜度处理。
pub(super) fn delta_seconds(value: &str) -> Option<&str> {
    let value = value.trim_matches('"');
    (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())).then_some(value)
}

pub(super) fn exact_asset_cache(
    relative_path: &str,
    web_script_policy: WebScriptPolicy,
) -> CachePolicy {
    // assets/ 也可包含原名发布的文件；只有当前技术栈产物的内容指纹文件名才能获得不可变缓存。
    let fingerprinted = relative_path.starts_with("assets/")
        && match web_script_policy {
            WebScriptPolicy::Dioxus => dioxus_fingerprinted_name(relative_path),
            WebScriptPolicy::Solid => solid_fingerprinted_name(relative_path),
        };
    if fingerprinted { CachePolicy::Immutable } else { CachePolicy::NoCache }
}

/// dx 逐字节输出 8 个哈希字节的十六进制，每字节 1 至 2 位。
fn dioxus_fingerprinted_name(relative_path: &str) -> bool {
    Path::new(relative_path)
        .file_stem()
        .and_then(|name| name.to_str())
        .and_then(|name| name.rsplit_once("-dxh"))
        .is_some_and(|(_, hash)| {
            (8..=16).contains(&hash.len()) && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

/// 构建侧递归枚举 `assets/`，按最后一级文件名保证形如 `名字-8位指纹.扩展名`。
fn solid_fingerprinted_name(relative_path: &str) -> bool {
    let Some(path_in_assets) = relative_path.strip_prefix("assets/") else {
        return false;
    };
    let Some(file_name) = path_in_assets.rsplit('/').next().filter(|name| !name.is_empty()) else {
        return false;
    };
    let Some((stem, extension)) = file_name.rsplit_once('.') else {
        return false;
    };
    !extension.is_empty()
        && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
        && stem.len() >= 10
        && stem.as_bytes()[0].is_ascii_alphanumeric()
        && stem
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        && stem.as_bytes()[stem.len() - 9] == b'-'
        && stem[stem.len() - 8..]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}
