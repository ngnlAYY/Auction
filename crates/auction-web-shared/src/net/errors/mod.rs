//! 稳定错误码的通用分类基础层。
//! 只识别两端共享的通用码（传输层三码、HTTP 兜底码及冲突/无效请求模式），业务码返回
//! `None`。本 crate 不含文案：surface 通过回调把 [`BaseErrorKind`] 映射为自己的译文。

/// 通用稳定错误码的分类；只描述语义，不携带文案。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseErrorKind {
    /// `network_error`：浏览器层网络失败。
    Network,
    /// `request_timeout`：共享传输层超时。
    Timeout,
    /// `invalid_response`：响应正文无法按契约解码。
    InvalidResponse,
    /// 未认证或凭据失效（`unauthenticated` / `invalid_credentials` / `csrf_unavailable` / `http_401`）。
    Unauthenticated,
    /// 权限不足或来源被禁止（`permission_denied` / `csrf_invalid` / `origin_forbidden` / `http_403`）。
    Forbidden,
    /// 资源不存在（`not_found` / `http_404`）。
    NotFound,
    /// 触发限流（`rate_limited` / `http_429`）。
    RateLimited,
    /// 服务端暂不可用（`storage_unavailable` / `temporarily_unavailable` / `internal_error` / `http_500` / `http_502` / `http_503` / `http_504`）。
    Unavailable,
    /// 功能不受支持（`feature_not_supported` / `http_501`）。
    FeatureNotSupported,
    /// 幂等命令冲突（`*_conflict` 后缀或 `stale_*` 前缀）。
    Conflict,
    /// 其他 `invalid_*` 请求错误与 `http_400`。
    InvalidRequest,
}

/// 把稳定错误码分类为通用类别；业务码返回 `None`。精确匹配优先于模式匹配，剩余
/// `invalid_*` 才落入 [`BaseErrorKind::InvalidRequest`]。
pub fn classify_base_error(code: &str) -> Option<BaseErrorKind> {
    match code {
        "network_error" => Some(BaseErrorKind::Network),
        "request_timeout" => Some(BaseErrorKind::Timeout),
        "invalid_response" => Some(BaseErrorKind::InvalidResponse),
        "unauthenticated" | "invalid_credentials" | "csrf_unavailable" | "http_401" => {
            Some(BaseErrorKind::Unauthenticated)
        }
        "permission_denied" | "csrf_invalid" | "origin_forbidden" | "http_403" => {
            Some(BaseErrorKind::Forbidden)
        }
        "not_found" | "http_404" => Some(BaseErrorKind::NotFound),
        "rate_limited" | "http_429" => Some(BaseErrorKind::RateLimited),
        "feature_not_supported" | "http_501" => Some(BaseErrorKind::FeatureNotSupported),
        "storage_unavailable"
        | "temporarily_unavailable"
        | "internal_error"
        | "http_500"
        | "http_502"
        | "http_503"
        | "http_504" => Some(BaseErrorKind::Unavailable),
        "invalid_request" | "http_400" => Some(BaseErrorKind::InvalidRequest),
        _ if code.ends_with("_conflict") || code.starts_with("stale_") => {
            Some(BaseErrorKind::Conflict)
        }
        _ if code.starts_with("invalid_") => Some(BaseErrorKind::InvalidRequest),
        _ => None,
    }
}

/// 基础码判定 + surface 回调取文案的统一入口。
/// 命中通用类别时返回 `Some(message(kind))`；业务码返回 `None`，由 surface 先匹配自己的
/// 业务码，未命中再调用本函数，最后兜底到自己的通用译文。
pub fn base_api_error_message(
    code: &str,
    message: impl Fn(BaseErrorKind) -> String,
) -> Option<String> {
    classify_base_error(code).map(message)
}
