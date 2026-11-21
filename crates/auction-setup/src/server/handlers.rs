//! 首次设置 HTTP handler 与请求校验。

use std::{sync::Arc, time::Duration};

use auction_config::{FirstRunConfig, FirstRunError, PostgresConnectionConfig};
use auction_contracts::{SetupErrorResponse, SetupRequest, SetupResponse};
use auction_storage::DatabaseStore;
use axum::{
    Json,
    extract::{FromRequest, Request, State},
    http::{HeaderMap, StatusCode},
};
use subtle::ConstantTimeEq;
use tokio::sync::MutexGuard;

use super::{
    FirstRunState, SetupDatabaseTarget, SetupFailure, SetupOperation, SetupResult, SetupStatus,
    access, remove_setup_token,
};
/// 仅验证部署码和操作状态，不接收配置、不连接数据库，也不消费令牌或建立全局已验证状态。
pub(super) async fn verify(
    State(state): State<Arc<FirstRunState>>,
    headers: HeaderMap,
) -> SetupResult {
    authorize(&state, &headers)?;
    let _operation = begin_operation(&state)?;
    Ok(Json(SetupResponse {
        status: "verified".to_owned(),
        initialize_and_start: state.policy.initialize_and_start,
    }))
}

pub(super) async fn validate(
    State(state): State<Arc<FirstRunState>>,
    request: Request,
) -> SetupResult {
    authorize(&state, request.headers())?;
    let _operation = begin_operation(&state)?;
    let Json(request) = Json::<SetupRequest>::from_request(request, &())
        .await
        .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid_input"))?;
    let options = config(&state, request);
    options.validate().map_err(config_error)?;
    check_database(&database_url(&options.database)?).await?;
    Ok(Json(SetupResponse {
        status: "validated".to_owned(),
        initialize_and_start: state.policy.initialize_and_start,
    }))
}

pub(super) async fn complete(
    State(state): State<Arc<FirstRunState>>,
    request: Request,
) -> SetupResult {
    authorize(&state, request.headers())?;
    let mut operation = begin_operation(&state)?;
    let confirmed_target = if state.policy.initialize_and_start {
        access::single_header(request.headers(), "x-setup-confirm-target")
            .ok()
            .flatten()
            .map(str::to_owned)
    } else {
        None
    };
    let Json(request) = Json::<SetupRequest>::from_request(request, &())
        .await
        .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid_input"))?;
    if state.policy.initialize_and_start {
        require_confirmed_target(&request, confirmed_target.as_deref())?;
    }
    let options = config(&state, request);
    options.validate().map_err(config_error)?;
    let database_url = database_url(&options.database)?;
    let confirmed_database = if state.policy.initialize_and_start {
        let target = SetupDatabaseTarget::from_database_url(&database_url)
            .ok_or_else(|| error(StatusCode::BAD_REQUEST, "invalid_input"))?;
        Some(target)
    } else {
        None
    };
    check_database(&database_url).await?;
    let output_dir = state.options.output_dir.clone();
    let initialize_and_start = state.policy.initialize_and_start;
    // `spawn_blocking` continues after the request future is cancelled; mark the operation unknown
    // before handoff so another request cannot race a still-running publication.
    operation.status = SetupStatus::OutcomeUnknown;
    let joined = tokio::task::spawn_blocking(move || {
        if initialize_and_start {
            options.write_managed_to(&output_dir)
        } else {
            options.write_to(&output_dir)
        }
    })
    .await;
    let written = match joined {
        Ok(written) => written,
        Err(_) => {
            operation.status = SetupStatus::OutcomeUnknown;
            if let Some(sender) = operation.shutdown.take() {
                let _ = sender.send(());
            }
            return Err(error(StatusCode::INTERNAL_SERVER_ERROR, "setup_outcome_unknown"));
        }
    };
    match written {
        // 发布和保存授权目标使用同一门禁，让监管进程按这次确认复核重读的配置。
        Ok(()) => {
            let token_file = state.token_file.clone();
            let removed =
                tokio::task::spawn_blocking(move || remove_setup_token(&token_file)).await;
            if !matches!(removed, Ok(Ok(()))) {
                operation.status = SetupStatus::OutcomeUnknown;
                if let Some(sender) = operation.shutdown.take() {
                    let _ = sender.send(());
                }
                return Err(error(StatusCode::INTERNAL_SERVER_ERROR, "setup_outcome_unknown"));
            }
            operation.status = SetupStatus::Completed(confirmed_database);
        }
        Err(FirstRunError::OutcomeUnknown) => {
            // 目录可能已经发布，保留终态直到人工检查并停止此进程，不能恢复为可重试状态。
            operation.status = SetupStatus::OutcomeUnknown;
            return Err(config_error(FirstRunError::OutcomeUnknown));
        }
        Err(error) => {
            operation.status = SetupStatus::Pending;
            return Err(config_error(error));
        }
    }
    if let Some(sender) = operation.shutdown.take() {
        let _ = sender.send(());
    }
    Ok(Json(SetupResponse {
        status: "completed".to_owned(),
        initialize_and_start: state.policy.initialize_and_start,
    }))
}

fn require_confirmed_target(
    request: &SetupRequest,
    confirmed_target: Option<&str>,
) -> Result<(), SetupFailure> {
    // 空库初始化的授权必须与将要发布的目标一致。先核对，再试连和写盘，确认头只包含端点与库名。
    let database = &request.database;
    let target = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("host", &database.host)
        .append_pair("port", &database.port.to_string())
        .append_pair("name", &database.name)
        .finish();
    if !request.confirm_initialize || confirmed_target != Some(target.as_str()) {
        return Err(error(StatusCode::BAD_REQUEST, "setup_confirmation_required"));
    }
    Ok(())
}

/// 三个接口先独立鉴权再获取同一非阻塞门禁；锁覆盖数据库等待、目录发布、终态与完成通知。
fn begin_operation(state: &FirstRunState) -> Result<MutexGuard<'_, SetupOperation>, SetupFailure> {
    let operation =
        state.operation.try_lock().map_err(|_| error(StatusCode::CONFLICT, "setup_busy"))?;
    match operation.status {
        SetupStatus::Pending => Ok(operation),
        SetupStatus::Completed(_) => Err(error(StatusCode::CONFLICT, "setup_completed")),
        SetupStatus::OutcomeUnknown => {
            Err(error(StatusCode::INTERNAL_SERVER_ERROR, "setup_outcome_unknown"))
        }
    }
}

fn authorize(
    state: &FirstRunState,
    headers: &HeaderMap,
) -> Result<(), (StatusCode, Json<SetupErrorResponse>)> {
    let provided = access::single_header(headers, "x-setup-token").ok().flatten().unwrap_or("");
    if provided.len() != state.token.len()
        || !bool::from(provided.as_bytes().ct_eq(state.token.as_bytes()))
    {
        return Err(error(StatusCode::FORBIDDEN, "setup_auth"));
    }
    Ok(())
}

pub(super) fn config(state: &FirstRunState, request: SetupRequest) -> FirstRunConfig {
    FirstRunConfig {
        database: PostgresConnectionConfig {
            host: request.database.host,
            port: request.database.port,
            username: request.database.username,
            password: request.database.password,
            name: request.database.name,
        },
        admin_actor: request.admin_actor,
        admin_secret: request.admin_secret,
        public_origin: request.public_origin,
        admin_origin: request.admin_origin,
        public_listener_port: request.public_listener_port,
        admin_listener_port: request.admin_listener_port,
        metrics_enabled: request.metrics_enabled,
        schedule: auction_config::AuctionScheduleConfig {
            enabled: request.schedule_enabled,
            timezone: request.schedule.timezone,
            preview_day: request.schedule.preview_day,
            preview_time: request.schedule.preview_time,
            open_day: request.schedule.open_day,
            open_time: request.schedule.open_time,
            nominal_end_day: request.schedule.nominal_end_day,
            nominal_end_time: request.schedule.nominal_end_time,
            // 首次表单不开放 world 或 runtime 接管；沿用 main、isekai 的部署默认值。
            worlds: vec!["main".to_owned(), "isekai".to_owned()],
            // 首次设置不开放早鸟期；保持默认关闭，后续在公共配置或 runtime 中启用。
            early_bird: Default::default(),
        },
        start_price: request.start_price,
        log_filter: request.log_filter,
        worker_concurrency: request.worker_concurrency,
        worker_poll_interval_seconds: request.worker_poll_interval_seconds,
        realtime_poll_interval_millis: request.realtime_poll_interval_millis,
        control_group: state.options.control_group.clone(),
        control_uid: state.options.control_uid,
    }
}

pub(super) fn database_url(database: &PostgresConnectionConfig) -> Result<String, SetupFailure> {
    database.connection_url().map_err(|_| error(StatusCode::BAD_REQUEST, "invalid_input"))
}

pub(super) async fn check_database(database_url: &str) -> Result<(), SetupFailure> {
    let maintenance = auction_config::DatabaseProfilesConfig::default().maintenance;
    let profile = auction_storage::DatabaseConnectionProfile::maintenance(
        auction_storage::DatabaseConnectionPurpose::SetupCheck,
        maintenance.max_connections,
        maintenance.acquire_timeout_millis,
    );
    match tokio::time::timeout(
        Duration::from_secs(8),
        DatabaseStore::connect(database_url, profile),
    )
    .await
    {
        Ok(Ok(store)) => {
            // 首次空库迁移要求无其他客户端；主动关闭连通性检查池，不遗留后台关闭窗口。
            store.postgres_pool().close().await;
            Ok(())
        }
        _ => Err(error(StatusCode::SERVICE_UNAVAILABLE, "database_unavailable")),
    }
}

pub(super) fn config_error(error_value: FirstRunError) -> (StatusCode, Json<SetupErrorResponse>) {
    match error_value {
        FirstRunError::InvalidInput => error(StatusCode::BAD_REQUEST, "invalid_input"),
        FirstRunError::InvalidConfig => error(StatusCode::BAD_REQUEST, "configuration_invalid"),
        FirstRunError::AlreadyExists => error(StatusCode::CONFLICT, "setup_locked"),
        FirstRunError::Io => error(StatusCode::INTERNAL_SERVER_ERROR, "setup_io"),
        FirstRunError::OutcomeUnknown => {
            error(StatusCode::INTERNAL_SERVER_ERROR, "setup_outcome_unknown")
        }
    }
}

pub(super) fn error(
    status: StatusCode,
    code: &'static str,
) -> (StatusCode, Json<SetupErrorResponse>) {
    (status, Json(SetupErrorResponse { code: code.to_owned() }))
}
