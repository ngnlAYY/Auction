//! 稳定错误码的通用分类基础层。
//!
//! 只识别两端共享、无业务语义的通用码：传输层三码（见 `transport` 模块）、HTTP 状态
//! 兜底码和通用的冲突/无效请求模式；业务码一律返回 `None`，交由 surface 的业务码
//! 分支处理。文案不进入本层——surface 通过回调把 [`BaseErrorKind`] 映射为自己的
//! `t!` 译文（与 `Pager` 的 props 注入同一模式），本 crate 不含任何文案。

/// 通用稳定错误码的分类。
///
/// 分类只描述错误的通用语义，不携带文案；surface 按类别取自己的本地化提示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseErrorKind {
    /// `network_error`：浏览器层网络失败。
    Network,
    /// `request_timeout`：共享传输层超时。
    Timeout,
    /// `invalid_response`：响应正文无法按契约解码。
    InvalidResponse,
    /// 未认证或凭据失效（`unauthenticated` / `invalid_credentials` / `csrf_unavailable`
    /// / `http_401`）。
    Unauthenticated,
    /// 权限不足或来源被禁止（`permission_denied` / `csrf_invalid` / `origin_forbidden`
    /// / `http_403`）。
    Forbidden,
    /// 资源不存在（`not_found` / `http_404`）。
    NotFound,
    /// 触发限流（`rate_limited` / `http_429`）。
    RateLimited,
    /// 服务端暂不可用（`storage_unavailable` / `temporarily_unavailable` /
    /// `internal_error` / `http_500` / `http_502` / `http_503` / `http_504`）。
    Unavailable,
    /// 功能不受支持（`feature_not_supported` / `http_501`）。
    FeatureNotSupported,
    /// 幂等命令冲突（`*_conflict` 后缀或 `stale_*` 前缀）。
    Conflict,
    /// 其他 `invalid_*` 请求错误与 `http_400`。
    InvalidRequest,
}

/// 把稳定错误码分类为通用类别；业务码返回 `None`。
///
/// 精确匹配优先于模式匹配：`invalid_response` / `invalid_credentials` 等先命中各自的
/// 类别，剩余的 `invalid_*` 才落入 [`BaseErrorKind::InvalidRequest`]。
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
///
/// `code` 命中通用类别时返回 `Some(message(kind))`；业务码返回 `None`，由 surface 的
/// 业务码分支或兜底文案接管。surface 的组合顺序建议：先匹配自己的业务码，未命中再
/// 调用本函数，最后 `unwrap_or_else` 到自己的通用兜底译文。
///
/// 用法示意（下游迁移时参照）：
///
/// ```ignore
/// pub fn admin_api_error_message(code: &str) -> String {
///     match code {
///         "equipment_conflict" => return t!("admin-error-equipment-conflict"),
///         // …其余 admin 业务码…
///         _ => {}
///     }
///     base_api_error_message(code, |kind| match kind {
///         BaseErrorKind::Network => t!("admin-error-network"),
///         BaseErrorKind::Timeout => t!("admin-error-request-timeout"),
///         // …其余通用类别…
///     })
///     .unwrap_or_else(|| t!("admin-error-generic"))
/// }
/// ```
pub fn base_api_error_message(
    code: &str,
    message: impl Fn(BaseErrorKind) -> String,
) -> Option<String> {
    classify_base_error(code).map(message)
}
