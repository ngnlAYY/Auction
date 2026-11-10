//! 旧 v1 写接口共用的严格 JSON 请求边界。
//! 该边界在进入 handler 前统一验证媒体类型、路由级 body 上限、JSON 语法、未知字段之外的
//! 单值约束；具体 DTO 的 `deny_unknown_fields` 仍由 handler 的类型化反序列化负责。读取后会把
//! 原始字节放回请求，避免 HTTP 层出现第二套 DTO owner。

use std::error::Error as _;

use auction_contracts::ApiErrorResponse;
use axum::{
    Json,
    body::{Body, Bytes, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use http_body_util::LengthLimitError;
use serde::de::DeserializeOwned;

const POST: u8 = 1 << 0;
const PUT: u8 = 1 << 1;
const PATCH: u8 = 1 << 2;
const DELETE: u8 = 1 << 3;

/// 一个路由上需要严格 JSON 解码的方法及其字节上限。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Policy {
    limit: usize,
    methods: u8,
    invalid_json_code: &'static str,
    invalid_json_message: &'static str,
}

impl Policy {
    const fn new(limit: usize, methods: u8) -> Self {
        Self {
            limit,
            methods,
            invalid_json_code: "invalid_json",
            invalid_json_message: "请求 JSON 无效",
        }
    }

    const fn with_invalid_json(mut self, code: &'static str, message: &'static str) -> Self {
        self.invalid_json_code = code;
        self.invalid_json_message = message;
        self
    }

    fn applies_to(self, method: &Method) -> bool {
        let method = if *method == Method::POST {
            POST
        } else if *method == Method::PUT {
            PUT
        } else if *method == Method::PATCH {
            PATCH
        } else if *method == Method::DELETE {
            DELETE
        } else {
            0
        };
        self.methods & method != 0
    }
}

pub(crate) const fn post(limit: usize) -> Policy {
    Policy::new(limit, POST)
}

/// 为已经发布独立 400 code 的旧 POST 路由保留其 JSON 语法错误外壳。
pub(crate) const fn post_with_invalid_json(
    limit: usize,
    code: &'static str,
    message: &'static str,
) -> Policy {
    Policy::new(limit, POST).with_invalid_json(code, message)
}

pub(crate) const fn put(limit: usize) -> Policy {
    Policy::new(limit, PUT)
}

pub(crate) const fn patch(limit: usize) -> Policy {
    Policy::new(limit, PATCH)
}

pub(crate) const fn post_put(limit: usize) -> Policy {
    Policy::new(limit, POST | PUT)
}

pub(crate) const fn post_patch(limit: usize) -> Policy {
    Policy::new(limit, POST | PATCH)
}

pub(crate) const fn put_delete(limit: usize) -> Policy {
    Policy::new(limit, PUT | DELETE)
}

pub(crate) const fn post_patch_delete(limit: usize) -> Policy {
    Policy::new(limit, POST | PATCH | DELETE)
}

/// 在认证和 route-specific path/query 校验之后读取严格 JSON，恢复旧 mutation 的可观察顺序。
/// 调用方必须移除前置 body extractor，由本方法独占正文大小、媒体类型、单值与 DTO 字段校验；
/// 原始 `Body` 不消费 `DefaultBodyLimit`，未认证请求不会提前暴露请求体分类。
pub(crate) async fn decode_after_guard<T>(
    headers: &HeaderMap,
    body: Body,
    limit: usize,
) -> Result<T, Box<Response>>
where
    T: DeserializeOwned,
{
    if !application_json(headers) {
        return Err(Box::new(error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "请求必须使用 JSON",
        )));
    }
    let (bytes, oversized) = match read_bounded_prefix(body, limit).await {
        Ok(result) => result,
        Err(()) => {
            return Err(Box::new(error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")));
        }
    };
    if oversized {
        // 旧 Go Decoder 会先报告上限内已经确定的 JSON/DTO 错误；只有必须继续读取才能完成
        // 当前值时，MaxBytesReader 的错误才覆盖为 413。这里使用有界前缀恢复同一可观察顺序。
        let response = match oversized_prefix_result::<T>(&bytes) {
            OversizedPrefixResult::InvalidJson => {
                error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
            }
            OversizedPrefixResult::BodyTooLarge => {
                error(StatusCode::PAYLOAD_TOO_LARGE, "body_too_large", "请求体过大")
            }
        };
        return Err(Box::new(response));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| Box::new(error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OversizedPrefixResult {
    InvalidJson,
    BodyTooLarge,
}

/// 只保留 `limit` 字节用于分类，并通过额外观察一个字节确认请求是否真正超限。
/// 单帧正文直接交给解码器；只有多帧才合并，不预分配整条路由上限或复制超限观察字节。
async fn read_bounded_prefix(body: Body, limit: usize) -> Result<(Bytes, bool), ()> {
    let mut stream = body.into_data_stream();
    let mut first = None;
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ())?;
        if chunk.is_empty() {
            continue;
        }
        let buffered = first.as_ref().map_or(bytes.len(), Bytes::len);
        let remaining = limit.saturating_sub(buffered);
        let copied = chunk.len().min(remaining);
        if let Some(first) = first.take() {
            if copied == 0 {
                return Ok((first, true));
            }
            bytes.reserve(buffered + copied);
            bytes.extend_from_slice(&first);
        } else if bytes.is_empty() && copied == chunk.len() {
            first = Some(chunk);
            continue;
        }
        bytes.extend_from_slice(&chunk[..copied]);
        if copied < chunk.len() {
            return Ok((bytes.into(), true));
        }
    }
    Ok((first.unwrap_or_else(|| bytes.into()), false))
}

/// 使用通用 JSON 流确定首值是否能在上限内闭合，再对完整首值执行目标 DTO 校验。
fn oversized_prefix_result<T>(bytes: &[u8]) -> OversizedPrefixResult
where
    T: DeserializeOwned,
{
    let mut values = serde_json::Deserializer::from_slice(bytes).into_iter::<serde_json::Value>();
    match values.next() {
        Some(Ok(_)) => {}
        Some(Err(error)) if error.is_eof() => return OversizedPrefixResult::BodyTooLarge,
        Some(Err(_)) => return OversizedPrefixResult::InvalidJson,
        None => return OversizedPrefixResult::BodyTooLarge,
    }

    let first_value_end = values.byte_offset();
    if serde_json::from_slice::<T>(&bytes[..first_value_end]).is_err() {
        return OversizedPrefixResult::InvalidJson;
    }
    match values.next() {
        None => OversizedPrefixResult::BodyTooLarge,
        Some(Err(error)) if error.is_eof() => OversizedPrefixResult::BodyTooLarge,
        Some(Ok(_) | Err(_)) => OversizedPrefixResult::InvalidJson,
    }
}

/// 在 route-specific `DefaultBodyLimit` 之外恢复旧版的稳定 400/413/415 JSON 语义。
pub(crate) async fn enforce(
    State(policy): State<Policy>,
    request: Request,
    next: Next,
) -> Response {
    if !policy.applies_to(request.method()) {
        return next.run(request).await;
    }
    if !application_json(request.headers()) {
        return error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "请求必须使用 JSON",
        );
    }

    let (parts, body) = request.into_parts();
    let bytes = match to_bytes(body, policy.limit).await {
        Ok(bytes) => bytes,
        Err(body_error) => {
            let too_large =
                body_error.source().is_some_and(|source| source.is::<LengthLimitError>());
            if too_large {
                return error(StatusCode::PAYLOAD_TOO_LARGE, "body_too_large", "请求体过大");
            }
            return error(
                StatusCode::BAD_REQUEST,
                policy.invalid_json_code,
                policy.invalid_json_message,
            );
        }
    };
    if serde_json::from_slice::<serde_json::Value>(&bytes).is_err() {
        return error(
            StatusCode::BAD_REQUEST,
            policy.invalid_json_code,
            policy.invalid_json_message,
        );
    }

    next.run(Request::from_parts(parts, Body::from(bytes))).await
}

/// 沿用旧 v1 对首个 Content-Type 的媒体类型校验，不扩展到 `+json` 子类型。
pub(crate) fn application_json(headers: &HeaderMap) -> bool {
    let Some(value) = headers.get(header::CONTENT_TYPE).and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Ok(media_type) = value.parse::<mime::Mime>() else {
        return false;
    };
    media_type.type_() == mime::APPLICATION
        && media_type.subtype() == mime::JSON
        && media_type.suffix().is_none()
}

fn error(status: StatusCode, code: &'static str, message: &'static str) -> Response {
    (
        status,
        Json(ApiErrorResponse {
            code: code.to_owned(),
            message: message.to_owned(),
            data: None,
            request_id: None,
        }),
    )
        .into_response()
}
