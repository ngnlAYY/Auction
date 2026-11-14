//! 静态资源与 CDN 的缓存策略。

use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::Response;
use rust_embed::EmbeddedFile;

use super::INDEX_HTML;

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
    let mut no_cache = false;
    for value in headers.get_all(header::CACHE_CONTROL) {
        let Ok(text) = value.to_str() else {
            return None;
        };
        let mut is_public = false;
        let mut malformed_freshness = false;
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
                malformed_freshness |= seconds.is_none();
                max_age = seconds;
            } else if name.eq_ignore_ascii_case("s-maxage") {
                malformed_freshness |= seconds.is_none();
                shared_max_age = seconds;
            } else if name.eq_ignore_ascii_case("stale-while-revalidate") {
                stale_while_revalidate = seconds;
            } else if name.eq_ignore_ascii_case("stale-if-error") {
                stale_if_error = seconds;
            }
        }
        // 新鲜度写坏的响应不猜测意图，按无资格处理。
        let freshness = if malformed_freshness { None } else { shared_max_age.or(max_age) };
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
    eligible
}

/// 只接受非负十进制秒数；`max-age` 历史上可能带引号，两种写法都按有效新鲜度处理。
pub(super) fn delta_seconds(value: &str) -> Option<&str> {
    let value = value.trim_matches('"');
    (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())).then_some(value)
}

pub(super) fn exact_asset_cache(relative_path: &str) -> CachePolicy {
    if relative_path == INDEX_HTML {
        // 入口文档必须再验证，但允许浏览器/CDN 保存，避免每次导航都完整回源。
        return CachePolicy::NoCache;
    }
    // Dioxus 把 JS、WASM、CSS 等构建资源全部放进 assets/ 并加内容指纹；该目录之外的文件名稳定，
    // 必须每次再验证，不能把全部非 index 文件都标为 immutable。
    if relative_path.starts_with("assets/") { CachePolicy::Immutable } else { CachePolicy::NoCache }
}

pub(super) fn favicon_file<A: rust_embed::RustEmbed>() -> Option<EmbeddedFile> {
    let path = A::iter().find(|path| {
        let path = path.as_ref();
        path.starts_with("assets/favicon-") && path.ends_with(".svg")
    })?;
    A::get(path.as_ref())
}
