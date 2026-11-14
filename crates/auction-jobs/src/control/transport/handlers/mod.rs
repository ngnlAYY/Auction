//! Worker control HTTP 路由处理器。

use std::sync::Arc;

use auction_application::external_accounts::ExternalAccountStatus;
use auction_application::operations::{
    WorkerControlMailProgressResult as MailProgressResult, WorkerEquipmentReadRequest,
    WorkerEquipmentReadResponse, WorkerRemoteReadError, WorkerRemoteReadPort,
    WorkerSentLookupRequest, WorkerSentLookupResponse,
};
use auction_observability::{MetricsError, ProcessMetrics};
use axum::body::Bytes;
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

use super::protocol::ResponseCode;
use super::protocol::invalid_request_id_response;
use super::protocol::request_id;
use super::protocol::response;
use super::protocol::{ReadyResult, TriggerPollCommand};
use super::{TriggerError, WORKER_CONTROL_METRICS_PATH, WorkerControlHandle};

pub(super) async fn live(
    State(handle): State<WorkerControlHandle>,
    headers: HeaderMap,
) -> Response {
    let request_id = match request_id_or_response(&headers) {
        Ok(request_id) => request_id,
        Err(response) => return *response,
    };
    response(StatusCode::OK, ResponseCode::Ok, "Worker 存活", Some(handle.live()), &request_id)
}

pub(super) async fn ready(
    State(handle): State<WorkerControlHandle>,
    headers: HeaderMap,
) -> Response {
    let request_id = match request_id_or_response(&headers) {
        Ok(request_id) => request_id,
        Err(response) => return *response,
    };
    let result = handle.ready().await;
    if !result.ready {
        return response(
            StatusCode::SERVICE_UNAVAILABLE,
            ResponseCode::NotReady,
            "Worker 尚未就绪",
            Option::<ReadyResult>::None,
            &request_id,
        );
    }
    response(StatusCode::OK, ResponseCode::Ok, "Worker 已就绪", Some(result), &request_id)
}

pub(super) async fn status(
    State(handle): State<WorkerControlHandle>,
    headers: HeaderMap,
) -> Response {
    let request_id = match request_id_or_response(&headers) {
        Ok(request_id) => request_id,
        Err(response) => return *response,
    };
    response(
        StatusCode::OK,
        ResponseCode::Ok,
        "Worker 状态可用",
        Some(handle.status()),
        &request_id,
    )
}

/// 仅返回当前进程的安全邮件观测；不触发远端读取或业务写入。
pub(super) async fn mail_progress(
    State(handle): State<WorkerControlHandle>,
    headers: HeaderMap,
) -> Response {
    let request_id = match request_id_or_response(&headers) {
        Ok(request_id) => request_id,
        Err(response) => return *response,
    };
    let snapshot = handle.mail_progress.snapshot();
    let Some(observed_at) = snapshot.observed_at.filter(|_| snapshot.validate()) else {
        return response(
            StatusCode::SERVICE_UNAVAILABLE,
            ResponseCode::Unavailable,
            "邮件运行观测不可用",
            Option::<MailProgressResult>::None,
            &request_id,
        );
    };
    response(
        StatusCode::OK,
        ResponseCode::Ok,
        "邮件运行观测已加载",
        Some(MailProgressResult { observed_at, pollers: snapshot.pollers }),
        &request_id,
    )
}

/// 仅返回当前进程的安全外部账号观测；不触发远端读取或业务写入。
pub(super) async fn external_accounts(
    State(handle): State<WorkerControlHandle>,
    headers: HeaderMap,
) -> Response {
    let request_id = match request_id_or_response(&headers) {
        Ok(request_id) => request_id,
        Err(response) => return *response,
    };
    match handle.external_accounts() {
        Some(status) => response(
            StatusCode::OK,
            ResponseCode::Ok,
            "外部账号运行观测可用",
            Some(status),
            &request_id,
        ),
        None => response(
            StatusCode::SERVICE_UNAVAILABLE,
            ResponseCode::CapabilityUnavailable,
            "外部账号运行观测不可用",
            Option::<ExternalAccountStatus>::None,
            &request_id,
        ),
    }
}

pub(super) async fn trigger_poll(
    State(handle): State<WorkerControlHandle>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request_id = match request_id_or_response(&headers) {
        Ok(request_id) => request_id,
        Err(response) => return *response,
    };
    let command: TriggerPollCommand = match json_request(&headers, &body, &request_id) {
        Ok(command) => command,
        Err(response) => return *response,
    };
    match handle.trigger_poll(command) {
        Ok(result) => response(
            StatusCode::ACCEPTED,
            ResponseCode::Accepted,
            "轮询唤醒已接受",
            Some(result),
            &request_id,
        ),
        Err(TriggerError::InvalidCommand) => response(
            StatusCode::BAD_REQUEST,
            ResponseCode::Invalid,
            "命令标识无效",
            Option::<()>::None,
            &request_id,
        ),
        Err(TriggerError::NotReady | TriggerError::NoPollers) => response(
            StatusCode::SERVICE_UNAVAILABLE,
            ResponseCode::NotReady,
            "Worker 尚未就绪",
            Option::<()>::None,
            &request_id,
        ),
        Err(TriggerError::Unavailable) => response(
            StatusCode::SERVICE_UNAVAILABLE,
            ResponseCode::Unavailable,
            "Worker 控制面不可用",
            Option::<()>::None,
            &request_id,
        ),
    }
}

pub(super) async fn metrics(
    Extension(metrics): Extension<Option<Arc<ProcessMetrics>>>,
    uri: Uri,
) -> Response {
    if uri.path() != WORKER_CONTROL_METRICS_PATH || uri.query().is_some() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(metrics) = metrics else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match metrics.encode() {
        Ok(body) => {
            ([(header::CONTENT_TYPE, ProcessMetrics::content_type())], body).into_response()
        }
        Err(MetricsError::Disabled) => StatusCode::NOT_FOUND.into_response(),
        Err(MetricsError::Registry(_) | MetricsError::Encode(_)) => {
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub(super) async fn not_found(headers: HeaderMap) -> Response {
    let request_id = request_id(&headers).unwrap_or_else(|_| Uuid::new_v4().to_string());
    response(
        StatusCode::NOT_FOUND,
        ResponseCode::Invalid,
        "请求的资源不存在",
        Option::<()>::None,
        &request_id,
    )
}

/// Admin 装备 URL 读取经 Worker 执行；凭据、请求门与暂停判定都在 Worker 进程。
/// 请求体只含世界与公开装备链接，响应不含 Cookie、正文或错误 cause。
pub(super) async fn equipment_read(
    Extension(reads): Extension<Option<Arc<dyn WorkerRemoteReadPort>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request_id = match request_id_or_response(&headers) {
        Ok(request_id) => request_id,
        Err(response) => return *response,
    };
    let request: WorkerEquipmentReadRequest = match json_request(&headers, &body, &request_id) {
        Ok(request) => request,
        Err(response) => return *response,
    };
    if !request.validate() {
        return response(
            StatusCode::BAD_REQUEST,
            ResponseCode::Invalid,
            "装备读取请求无效",
            Option::<()>::None,
            &request_id,
        );
    }
    let Some(reads) = reads else {
        return response(
            StatusCode::NOT_IMPLEMENTED,
            ResponseCode::CapabilityUnavailable,
            "Worker 未装配远端读取能力",
            Option::<WorkerEquipmentReadResponse>::None,
            &request_id,
        );
    };
    match reads.read_equipment(request).await {
        Ok(result) => {
            response(StatusCode::OK, ResponseCode::Ok, "装备读取完成", Some(result), &request_id)
        }
        Err(error) => remote_read_error_response(error, &request_id),
    }
}

/// CLI Sent 核查的远端读取；目标来自调用方本地数据库，Worker 只执行有界只读扫描。
pub(super) async fn sent_lookup(
    Extension(reads): Extension<Option<Arc<dyn WorkerRemoteReadPort>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request_id = match request_id_or_response(&headers) {
        Ok(request_id) => request_id,
        Err(response) => return *response,
    };
    let request: WorkerSentLookupRequest = match json_request(&headers, &body, &request_id) {
        Ok(request) => request,
        Err(response) => return *response,
    };
    if !request.validate() {
        return response(
            StatusCode::BAD_REQUEST,
            ResponseCode::Invalid,
            "Sent 核查请求无效",
            Option::<()>::None,
            &request_id,
        );
    }
    let Some(reads) = reads else {
        return response(
            StatusCode::NOT_IMPLEMENTED,
            ResponseCode::CapabilityUnavailable,
            "Worker 未装配远端读取能力",
            Option::<WorkerSentLookupResponse>::None,
            &request_id,
        );
    };
    match reads.lookup_sent(request).await {
        Ok(result) => {
            response(StatusCode::OK, ResponseCode::Ok, "Sent 核查完成", Some(result), &request_id)
        }
        Err(error) => remote_read_error_response(error, &request_id),
    }
}

/// 只在暂停生效时执行一次有界只读恢复扫描；不写入本地或远端。
pub(super) async fn recovery_scan(
    Extension(reads): Extension<Option<Arc<dyn WorkerRemoteReadPort>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request_id = match request_id_or_response(&headers) {
        Ok(request_id) => request_id,
        Err(response) => return *response,
    };
    let request: auction_application::operations::WorkerRecoveryScanRequest =
        match json_request(&headers, &body, &request_id) {
            Ok(request) => request,
            Err(response) => return *response,
        };
    if !request.validate() {
        return response(
            StatusCode::BAD_REQUEST,
            ResponseCode::Invalid,
            "恢复扫描请求无效",
            Option::<()>::None,
            &request_id,
        );
    }
    let Some(reads) = reads else {
        return response(
            StatusCode::NOT_IMPLEMENTED,
            ResponseCode::CapabilityUnavailable,
            "Worker 未装配远端读取能力",
            Option::<auction_application::operations::WorkerRecoveryScanResponse>::None,
            &request_id,
        );
    };
    match reads.recovery_scan(request).await {
        Ok(result) => {
            response(StatusCode::OK, ResponseCode::Ok, "恢复扫描完成", Some(result), &request_id)
        }
        Err(error) => remote_read_error_response(error, &request_id),
    }
}

/// 请求标识校验与 400 响应的一次性封装；handler 只关心成功路径。
fn request_id_or_response(headers: &HeaderMap) -> Result<String, Box<Response>> {
    request_id(headers).map_err(|()| Box::new(invalid_request_id_response()))
}

/// JSON 请求体的一次性校验：Content-Type 与严格反序列化共用同一组 415/400 响应。
fn json_request<T: serde::de::DeserializeOwned>(
    headers: &HeaderMap,
    body: &Bytes,
    request_id: &str,
) -> Result<T, Box<Response>> {
    if !json_content_type(headers) {
        return Err(Box::new(response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ResponseCode::Invalid,
            "请求必须使用 application/json",
            Option::<()>::None,
            request_id,
        )));
    }
    serde_json::from_slice(body).map_err(|_: serde_json::Error| {
        Box::new(response(
            StatusCode::BAD_REQUEST,
            ResponseCode::Invalid,
            "请求 JSON 无效",
            Option::<()>::None,
            request_id,
        ))
    })
}

fn json_content_type(headers: &HeaderMap) -> bool {
    headers.get(header::CONTENT_TYPE).is_some_and(|value| {
        value.to_str().ok().is_some_and(|value| {
            value.split(';').next().is_some_and(|value| value.trim() == "application/json")
        })
    })
}

/// 远端读取错误只保留稳定分类；暂停使用 409 以区别于“Worker 不可用”的 503。
fn remote_read_error_response(error: WorkerRemoteReadError, request_id: &str) -> Response {
    match error {
        WorkerRemoteReadError::Paused => response(
            StatusCode::CONFLICT,
            ResponseCode::Paused,
            "外部访问已暂停",
            Option::<()>::None,
            request_id,
        ),
        WorkerRemoteReadError::NotPaused => response(
            StatusCode::CONFLICT,
            ResponseCode::NotPaused,
            "恢复核对要求先暂停外部访问",
            Option::<()>::None,
            request_id,
        ),
        WorkerRemoteReadError::Invalid => response(
            StatusCode::BAD_REQUEST,
            ResponseCode::Invalid,
            "远端读取请求无效",
            Option::<()>::None,
            request_id,
        ),
        WorkerRemoteReadError::Unavailable => response(
            StatusCode::SERVICE_UNAVAILABLE,
            ResponseCode::CapabilityUnavailable,
            "远端读取暂时不可用",
            Option::<()>::None,
            request_id,
        ),
        WorkerRemoteReadError::Timeout => response(
            StatusCode::GATEWAY_TIMEOUT,
            ResponseCode::Unavailable,
            "远端读取超时",
            Option::<()>::None,
            request_id,
        ),
        WorkerRemoteReadError::Incompatible => response(
            StatusCode::BAD_GATEWAY,
            ResponseCode::Unavailable,
            "远端读取结果不受支持",
            Option::<()>::None,
            request_id,
        ),
    }
}

#[cfg(test)]
mod tests;
