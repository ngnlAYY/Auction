//! Worker control HTTP 路由处理器。

use super::*;

pub(super) async fn live(
    State(handle): State<WorkerControlHandle>,
    headers: HeaderMap,
) -> Response {
    let request_id = match request_id(&headers) {
        Ok(request_id) => request_id,
        Err(()) => return invalid_request_id_response(),
    };
    response(StatusCode::OK, ResponseCode::Ok, "Worker 存活", Some(handle.live()), &request_id)
}

pub(super) async fn ready(
    State(handle): State<WorkerControlHandle>,
    headers: HeaderMap,
) -> Response {
    let request_id = match request_id(&headers) {
        Ok(request_id) => request_id,
        Err(()) => return invalid_request_id_response(),
    };
    let result = handle.ready();
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
    let request_id = match request_id(&headers) {
        Ok(request_id) => request_id,
        Err(()) => return invalid_request_id_response(),
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
    let request_id = match request_id(&headers) {
        Ok(request_id) => request_id,
        Err(()) => return invalid_request_id_response(),
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

pub(super) async fn trigger_poll(
    State(handle): State<WorkerControlHandle>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request_id = match request_id(&headers) {
        Ok(request_id) => request_id,
        Err(()) => return invalid_request_id_response(),
    };
    if !headers.get(header::CONTENT_TYPE).is_some_and(|value| {
        value.to_str().ok().is_some_and(|value| {
            value.split(';').next().is_some_and(|value| value.trim() == "application/json")
        })
    }) {
        return response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ResponseCode::Invalid,
            "请求必须使用 application/json",
            Option::<()>::None,
            &request_id,
        );
    }
    let command: TriggerPollCommand = match serde_json::from_slice(&body) {
        Ok(command) => command,
        Err(_) => {
            return response(
                StatusCode::BAD_REQUEST,
                ResponseCode::Invalid,
                "请求 JSON 无效",
                Option::<()>::None,
                &request_id,
            );
        }
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
