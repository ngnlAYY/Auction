// 稳定错误码的通用分类基础层。
// 只识别两端共享的通用码（传输层三码、HTTP 兜底码及冲突/无效请求模式），业务码返回
// `null`。本模块不含文案：surface 通过回调把 BaseErrorKind 映射为自己的译文。

/** 通用稳定错误码的分类；只描述语义，不携带文案。 */
export type BaseErrorKind =
  | "network"
  | "timeout"
  | "invalid_response"
  | "unauthenticated"
  | "forbidden"
  | "not_found"
  | "rate_limited"
  | "unavailable"
  | "feature_not_supported"
  | "conflict"
  | "invalid_request";

/** 把稳定错误码分类为通用类别；业务码返回 `null`。精确匹配优先于模式匹配。 */
function classifyBaseError(code: string): BaseErrorKind | null {
  switch (code) {
    case "network_error":
      return "network";
    case "request_timeout":
      return "timeout";
    case "invalid_response":
      return "invalid_response";
    case "unauthenticated":
    case "invalid_credentials":
    case "csrf_unavailable":
    case "http_401":
      return "unauthenticated";
    case "permission_denied":
    case "csrf_invalid":
    case "origin_forbidden":
    case "http_403":
      return "forbidden";
    case "not_found":
    case "http_404":
      return "not_found";
    case "rate_limited":
    case "http_429":
      return "rate_limited";
    case "feature_not_supported":
    case "http_501":
      return "feature_not_supported";
    case "storage_unavailable":
    case "temporarily_unavailable":
    case "internal_error":
    case "http_500":
    case "http_502":
    case "http_503":
    case "http_504":
      return "unavailable";
    case "invalid_request":
    case "http_400":
      return "invalid_request";
    default:
      break;
  }
  if (code.endsWith("_conflict") || code.startsWith("stale_")) {
    return "conflict";
  }
  if (code.startsWith("invalid_")) {
    return "invalid_request";
  }
  return null;
}

/** 基础码判定 + surface 回调取文案的统一入口；业务码返回 `null`。 */
export function baseApiErrorMessage(
  code: string,
  message: (kind: BaseErrorKind) => string,
): string | null {
  const kind = classifyBaseError(code);
  return kind === null ? null : message(kind);
}
