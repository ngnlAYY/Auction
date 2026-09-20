//! 初始化页面的令牌门禁、数据库连通性检查与单次提交。

use std::{fs, io::ErrorKind, path::PathBuf, sync::Arc, time::Duration};

use auction_config::{FirstRunConfig, FirstRunError, PostgresConnectionConfig};
use auction_contracts::{SetupErrorResponse, SetupRequest, SetupResponse};
use auction_storage::DatabaseStore;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use rust_embed::RustEmbed;
use subtle::ConstantTimeEq;
use tokio::{
    net::TcpListener,
    sync::{Mutex, MutexGuard, oneshot},
};
use uuid::Uuid;

/// 只由装配根提供的本地初始化边界；不读取任何角色配置。
pub struct FirstRunServerOptions {
    pub output_dir: PathBuf,
    pub control_group: String,
    pub control_uid: u32,
}

struct FirstRunState {
    options: FirstRunServerOptions,
    token: String,
    operation: Mutex<SetupStatus>,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
}

/// 终态只能在同一操作锁下写入；结果不确定也禁止本进程继续校验或写入。
enum SetupStatus {
    Pending,
    Completed,
    OutcomeUnknown,
}

type SetupFailure = (StatusCode, Json<SetupErrorResponse>);
type SetupResult = Result<Json<SetupResponse>, SetupFailure>;

/// 启动仅绑定 127.0.0.1 的初始化进程；完成写入后退出，不自动进入正式 Admin 模式。
pub async fn run_first_run<A: RustEmbed + 'static>(
    options: FirstRunServerOptions,
) -> Result<(), &'static str> {
    match fs::symlink_metadata(&options.output_dir) {
        Ok(_) => return Err("setup output directory already exists"),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(_) => return Err("cannot inspect setup output directory"),
    }
    let listener =
        TcpListener::bind("127.0.0.1:8091").await.map_err(|_| "cannot bind setup listener")?;
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let (shutdown_sender, shutdown_receiver) = oneshot::channel();
    let state = Arc::new(FirstRunState {
        options,
        token,
        operation: Mutex::new(SetupStatus::Pending),
        shutdown: Mutex::new(Some(shutdown_sender)),
    });
    let app = Router::new()
        .route("/api/v1/setup/verify", post(verify))
        .route("/api/v1/setup/validate", post(validate))
        .route("/api/v1/setup/complete", post(complete))
        .fallback(auction_http::serve_admin_setup_asset::<A>)
        .layer(DefaultBodyLimit::max(8 * 1024))
        .with_state(Arc::clone(&state));

    eprintln!("首次部署页面：http://127.0.0.1:8091/setup");
    eprintln!("一次性设置令牌：{}", state.token);
    eprintln!("远程主机请先建立 SSH 本地端口转发；不要公开转发 8091。");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = shutdown_receiver => {},
                _ = tokio::signal::ctrl_c() => {},
            }
        })
        .await
        .map_err(|_| "setup HTTP server failed")?;
    match *state.operation.lock().await {
        SetupStatus::Completed => Ok(()),
        SetupStatus::Pending => Err("setup was interrupted"),
        SetupStatus::OutcomeUnknown => Err("setup output requires manual verification"),
    }
}

/// 仅验证部署码和操作状态，不接收配置、不连接数据库，也不消费令牌或建立全局已验证状态。
async fn verify(State(state): State<Arc<FirstRunState>>, headers: HeaderMap) -> SetupResult {
    authorize(&state, &headers)?;
    let _operation = begin_operation(&state)?;
    Ok(Json(SetupResponse { status: "verified".to_owned() }))
}

async fn validate(
    State(state): State<Arc<FirstRunState>>,
    headers: HeaderMap,
    request: Result<Json<SetupRequest>, JsonRejection>,
) -> SetupResult {
    authorize(&state, &headers)?;
    let _operation = begin_operation(&state)?;
    let Json(request) = request.map_err(|_| error(StatusCode::BAD_REQUEST, "invalid_input"))?;
    let options = config(&state, request);
    options.validate().map_err(config_error)?;
    check_database(&options.database).await?;
    Ok(Json(SetupResponse { status: "validated".to_owned() }))
}

async fn complete(
    State(state): State<Arc<FirstRunState>>,
    headers: HeaderMap,
    request: Result<Json<SetupRequest>, JsonRejection>,
) -> SetupResult {
    authorize(&state, &headers)?;
    let mut operation = begin_operation(&state)?;
    let Json(request) = request.map_err(|_| error(StatusCode::BAD_REQUEST, "invalid_input"))?;
    let options = config(&state, request);
    options.validate().map_err(config_error)?;
    check_database(&options.database).await?;
    match options.write_to(&state.options.output_dir) {
        Ok(()) => *operation = SetupStatus::Completed,
        Err(FirstRunError::OutcomeUnknown) => {
            // 目录可能已经发布，保留终态直到人工检查并停止此进程，不能恢复为可重试状态。
            *operation = SetupStatus::OutcomeUnknown;
            return Err(config_error(FirstRunError::OutcomeUnknown));
        }
        Err(error) => return Err(config_error(error)),
    }
    if let Some(sender) = state.shutdown.lock().await.take() {
        let _ = sender.send(());
    }
    Ok(Json(SetupResponse { status: "completed".to_owned() }))
}

/// 三个接口先独立鉴权再获取同一非阻塞门禁；锁覆盖数据库等待、目录发布和终态写入。
fn begin_operation(state: &FirstRunState) -> Result<MutexGuard<'_, SetupStatus>, SetupFailure> {
    let operation =
        state.operation.try_lock().map_err(|_| error(StatusCode::CONFLICT, "setup_busy"))?;
    match *operation {
        SetupStatus::Pending => Ok(operation),
        SetupStatus::Completed => Err(error(StatusCode::CONFLICT, "setup_completed")),
        SetupStatus::OutcomeUnknown => {
            Err(error(StatusCode::INTERNAL_SERVER_ERROR, "setup_outcome_unknown"))
        }
    }
}

fn authorize(
    state: &FirstRunState,
    headers: &HeaderMap,
) -> Result<(), (StatusCode, Json<SetupErrorResponse>)> {
    let provided = headers.get("x-setup-token").and_then(|value| value.to_str().ok()).unwrap_or("");
    if provided.len() != state.token.len()
        || !bool::from(provided.as_bytes().ct_eq(state.token.as_bytes()))
    {
        return Err(error(StatusCode::FORBIDDEN, "setup_auth"));
    }
    Ok(())
}

fn config(state: &FirstRunState, request: SetupRequest) -> FirstRunConfig {
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
        metrics_enabled: request.metrics_enabled,
        schedule_enabled: request.schedule_enabled,
        start_price: request.start_price,
        log_filter: request.log_filter,
        worker_concurrency: request.worker_concurrency,
        worker_poll_interval_seconds: request.worker_poll_interval_seconds,
        realtime_poll_interval_millis: request.realtime_poll_interval_millis,
        control_group: state.options.control_group.clone(),
        control_uid: state.options.control_uid,
    }
}

async fn check_database(
    database: &PostgresConnectionConfig,
) -> Result<(), (StatusCode, Json<SetupErrorResponse>)> {
    let database_url =
        database.connection_url().map_err(|_| error(StatusCode::BAD_REQUEST, "invalid_input"))?;
    match tokio::time::timeout(Duration::from_secs(8), DatabaseStore::connect(&database_url)).await
    {
        Ok(Ok(_)) => Ok(()),
        _ => Err(error(StatusCode::SERVICE_UNAVAILABLE, "database_unavailable")),
    }
}

fn config_error(error_value: FirstRunError) -> (StatusCode, Json<SetupErrorResponse>) {
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

fn error(status: StatusCode, code: &'static str) -> (StatusCode, Json<SetupErrorResponse>) {
    (status, Json(SetupErrorResponse { code: code.to_owned() }))
}
