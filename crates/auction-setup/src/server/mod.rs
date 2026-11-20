//! 初始化页面的令牌门禁、数据库连通性检查与单次提交。

use std::{fs, io::ErrorKind, net::IpAddr, path::PathBuf, sync::Arc, time::Duration};

use auction_config::{FirstRunConfig, FirstRunError, PostgresConnectionConfig};
use auction_contracts::{SetupErrorResponse, SetupRequest, SetupResponse};
use auction_http::WebScriptPolicy;
use auction_storage::{DatabaseStore, postgres_connect_options};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, FromRequest, Request, State},
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

mod access;
mod lifecycle;
pub use access::{FirstRunPolicy, RemoteSetupAccess};

/// 设置服务退出的确定结果。合并部署携带授权目标进入后续初始化。
pub enum FirstRunOutcome {
    /// 配置目录已发布。分体部署只发布配置，因此没有初始化目标。
    Completed {
        confirmed_database: Option<SetupDatabaseTarget>,
    },
    Interrupted,
    OutcomeUnknown,
}

/// 供首次初始化比对的数据库身份，只保存 TCP 主机、端口和库名。
#[derive(PartialEq, Eq)]
pub struct SetupDatabaseTarget {
    host: String,
    port: u16,
    name: String,
}

impl SetupDatabaseTarget {
    /// 按驱动实际使用的连接选项取目标，应用查询参数覆盖与 IP 地址规范化。
    pub fn from_database_url(database_url: &str) -> Option<Self> {
        let options = postgres_connect_options(database_url).ok()?;
        if options.get_socket().is_some() {
            return None;
        }
        let host = options.get_host();
        let host = host
            .parse::<IpAddr>()
            .map_or_else(|_error| host.to_ascii_lowercase(), |address| address.to_string());
        Some(Self { host, port: options.get_port(), name: options.get_database()?.to_owned() })
    }
}

/// 只由装配根提供的本地初始化边界；不读取任何角色配置。
pub struct FirstRunServerOptions {
    pub output_dir: PathBuf,
    pub control_group: String,
    pub control_uid: u32,
}

struct FirstRunState {
    options: FirstRunServerOptions,
    token: String,
    policy: Arc<FirstRunPolicy>,
    operation: Mutex<SetupOperation>,
}

/// 门禁中间件与处理 handler 共享同一份只读策略；脚本 CSP 由装配根按 surface 显式选择。
#[derive(Clone)]
struct SetupBoundary {
    policy: Arc<FirstRunPolicy>,
    web_script_policy: WebScriptPolicy,
}

/// 首次设置路由的唯一装配入口；测试与运行期共用，避免测试绕过门禁或策略接线。
fn first_run_router<A: RustEmbed + 'static>(
    state: Arc<FirstRunState>,
    web_script_policy: WebScriptPolicy,
) -> Router {
    let boundary = SetupBoundary { policy: Arc::clone(&state.policy), web_script_policy };
    Router::new()
        .route("/api/v1/setup/verify", post(verify))
        .route("/api/v1/setup/validate", post(validate))
        .route("/api/v1/setup/complete", post(complete))
        .fallback(move |request| {
            auction_http::serve_admin_setup_asset::<A>(request, web_script_policy)
        })
        .layer(DefaultBodyLimit::max(8 * 1024))
        .layer(axum::middleware::from_fn_with_state(boundary, access::enforce))
        .with_state(state)
}

/// 终态与完成通知由同一操作锁拥有，避免发布后为发送通知再次异步取锁。
struct SetupOperation {
    status: SetupStatus,
    shutdown: Option<oneshot::Sender<()>>,
}

/// 终态只能在同一操作锁下写入；结果不确定也禁止本进程继续校验或写入。
enum SetupStatus {
    Pending,
    Completed(Option<SetupDatabaseTarget>),
    OutcomeUnknown,
}

type SetupFailure = (StatusCode, Json<SetupErrorResponse>);
type SetupResult = Result<Json<SetupResponse>, SetupFailure>;

/// 启动仅绑定 127.0.0.1 的初始化进程；完成写入后退出，不自动进入正式 Admin 模式。
/// `web_script_policy` 由装配根按设置页实际使用的前端技术栈显式给出。
pub async fn run_first_run<A: RustEmbed + 'static>(
    options: FirstRunServerOptions,
    web_script_policy: WebScriptPolicy,
) -> Result<(), &'static str> {
    match run_first_run_with_policy::<A>(options, FirstRunPolicy::default(), web_script_policy)
        .await?
    {
        FirstRunOutcome::Completed { .. } => Ok(()),
        FirstRunOutcome::Interrupted => Err("setup was interrupted"),
        FirstRunOutcome::OutcomeUnknown => Err("setup output requires manual verification"),
    }
}

/// 通过独立策略启用受信 HTTPS 代理或合并入口的初始化确认，不挂载普通 Admin 写接口。
/// `web_script_policy` 与其他绑定参数一样在启动时确定，并作用于所有首次设置响应。
pub async fn run_first_run_with_policy<A: RustEmbed + 'static>(
    options: FirstRunServerOptions,
    policy: FirstRunPolicy,
    web_script_policy: WebScriptPolicy,
) -> Result<FirstRunOutcome, &'static str> {
    policy.validate()?;
    let policy = Arc::new(policy);
    let signals = auction_process::TerminationSignals::subscribe()
        .map_err(|_| "cannot subscribe setup termination signals")?;
    match fs::symlink_metadata(&options.output_dir) {
        Ok(_) => return Err("setup output directory already exists"),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(_) => return Err("cannot inspect setup output directory"),
    }
    let listener =
        TcpListener::bind(policy.listen).await.map_err(|_| "cannot bind setup listener")?;
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let (shutdown_sender, shutdown_receiver) = oneshot::channel();
    let state = Arc::new(FirstRunState {
        options,
        token,
        policy: Arc::clone(&policy),
        operation: Mutex::new(SetupOperation {
            status: SetupStatus::Pending,
            shutdown: Some(shutdown_sender),
        }),
    });
    let app = first_run_router::<A>(Arc::clone(&state), web_script_policy);

    let origin = policy
        .remote
        .as_ref()
        .map_or_else(|| format!("http://{}", policy.listen), |remote| remote.origin.clone());
    eprintln!("首次部署页面：{origin}/setup");
    eprintln!("一次性设置令牌：{}", state.token);
    eprintln!("仅使用 SSH 隧道或已显式授权的 HTTPS 代理；不要公开明文初始化监听器。");

    let reason = lifecycle::serve(listener, app, signals, shutdown_receiver).await?;
    // 强制收口后不再次无界等待操作锁；无法确认终态时禁止后续迁移和角色启动。
    let Ok(mut operation) = state.operation.try_lock() else {
        return Ok(FirstRunOutcome::OutcomeUnknown);
    };
    match &mut operation.status {
        SetupStatus::Completed(target)
            if matches!(reason, lifecycle::ShutdownReason::Completed) =>
        {
            Ok(FirstRunOutcome::Completed { confirmed_database: target.take() })
        }
        SetupStatus::OutcomeUnknown => Ok(FirstRunOutcome::OutcomeUnknown),
        _ => Ok(FirstRunOutcome::Interrupted),
    }
}

/// 仅验证部署码和操作状态，不接收配置、不连接数据库，也不消费令牌或建立全局已验证状态。
async fn verify(State(state): State<Arc<FirstRunState>>, headers: HeaderMap) -> SetupResult {
    authorize(&state, &headers)?;
    let _operation = begin_operation(&state)?;
    Ok(Json(SetupResponse {
        status: "verified".to_owned(),
        initialize_and_start: state.policy.initialize_and_start,
    }))
}

async fn validate(State(state): State<Arc<FirstRunState>>, request: Request) -> SetupResult {
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

async fn complete(State(state): State<Arc<FirstRunState>>, request: Request) -> SetupResult {
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
    let written = if state.policy.initialize_and_start {
        options.write_managed_to(&state.options.output_dir)
    } else {
        options.write_to(&state.options.output_dir)
    };
    match written {
        // 发布和保存授权目标使用同一门禁，让监管进程按这次确认复核重读的配置。
        Ok(()) => operation.status = SetupStatus::Completed(confirmed_database),
        Err(FirstRunError::OutcomeUnknown) => {
            // 目录可能已经发布，保留终态直到人工检查并停止此进程，不能恢复为可重试状态。
            operation.status = SetupStatus::OutcomeUnknown;
            return Err(config_error(FirstRunError::OutcomeUnknown));
        }
        Err(error) => return Err(config_error(error)),
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

fn database_url(database: &PostgresConnectionConfig) -> Result<String, SetupFailure> {
    database.connection_url().map_err(|_| error(StatusCode::BAD_REQUEST, "invalid_input"))
}

async fn check_database(database_url: &str) -> Result<(), SetupFailure> {
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

#[cfg(test)]
mod tests {
    use std::{borrow::Cow, net::SocketAddr, sync::Arc, time::Duration};

    use auction_contracts::{SetupDatabaseRequest, SetupRequest, SetupScheduleRequest};
    use auction_http::WebScriptPolicy;
    use axum::{
        Json,
        body::{Body, to_bytes},
        extract::{Request, State},
        http::StatusCode,
        response::IntoResponse,
        serve::{Listener, ListenerExt},
    };
    use rust_embed::{EmbeddedFile, Metadata, RustEmbed};
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, duplex};
    use tokio::sync::Mutex;

    use super::{
        FirstRunPolicy, FirstRunServerOptions, FirstRunState, SetupOperation, SetupStatus,
        complete, first_run_router,
    };

    const DATABASE_TARGET: &str = "host=%3A%3A1&port=5432&name=%E6%8B%8D%E5%8D%96+%26+%25";
    const DIOXUS_CSP: &str = "default-src 'self'; script-src 'self' 'unsafe-eval' \
                              'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; \
                              object-src 'none'; base-uri 'self'; form-action 'self'; \
                              frame-ancestors 'none'";
    const SOLID_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' \
                             'unsafe-inline'; object-src 'none'; base-uri 'self'; \
                             form-action 'self'; frame-ancestors 'none'";
    const SETUP_PAGE_REQUEST: &str =
        "GET /setup HTTP/1.1\r\nhost: 127.0.0.1:8091\r\nconnection: close\r\n\r\n";
    const TARGET_MISMATCH_RESPONSE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../auction-contracts/tests/fixtures/admin/setup-confirm-target-mismatch.json"
    ));

    struct NoAssets;

    impl RustEmbed for NoAssets {
        fn get(_path: &str) -> Option<EmbeddedFile> {
            None
        }

        fn iter() -> impl Iterator<Item = Cow<'static, str>> {
            std::iter::empty()
        }
    }

    const SOLID_INDEX: &[u8] = br#"<!doctype html><html lang="en"><head><meta charset="utf-8"><link rel="modulepreload" href="/assets/index-B2UikXgQ.js" crossorigin><link rel="modulepreload" href="/assets/rolldown-runtime-DK3Fl9T5.js" crossorigin><link rel="stylesheet" href="/assets/index-ZX7NKpK-.css" crossorigin></head><body><div id="app"></div><script type="module" src="/assets/index-B2UikXgQ.js" crossorigin></script></body></html>"#;
    const SOLID_INDEX_JS: &[u8] = b"console.log('solid');";
    const SOLID_RUNTIME_JS: &[u8] = b"console.log('runtime');";
    const SOLID_STYLE_CSS: &[u8] = b"body {}";

    /// Solid 产物的最小替身：文件名是构建侧保证的 `名字-8位指纹.扩展名` 形状。
    struct SolidSetupAssets;

    impl RustEmbed for SolidSetupAssets {
        fn get(path: &str) -> Option<EmbeddedFile> {
            let (data, mime) = match path {
                "index.html" => (SOLID_INDEX, "text/html"),
                "assets/index-B2UikXgQ.js" => (SOLID_INDEX_JS, "text/javascript"),
                "assets/rolldown-runtime-DK3Fl9T5.js" => (SOLID_RUNTIME_JS, "text/javascript"),
                "assets/index-ZX7NKpK-.css" => (SOLID_STYLE_CSS, "text/css"),
                _ => return None,
            };
            Some(EmbeddedFile {
                data: Cow::Borrowed(data),
                metadata: Metadata::__rust_embed_new([0; 32], None, None, mime),
            })
        }

        fn iter() -> impl Iterator<Item = Cow<'static, str>> {
            [
                "index.html",
                "assets/index-B2UikXgQ.js",
                "assets/index-ZX7NKpK-.css",
                "assets/rolldown-runtime-DK3Fl9T5.js",
            ]
            .into_iter()
            .map(Cow::Borrowed)
        }
    }

    /// 生产路由需要真实监听器；测试用一次性内存管道替代套接字，仍经 axum::serve 与连接信息注入。
    struct MemoryListener<I> {
        io: Option<I>,
        addr: SocketAddr,
    }

    impl<I> Listener for MemoryListener<I>
    where
        I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        type Io = I;
        type Addr = SocketAddr;

        async fn accept(&mut self) -> (Self::Io, Self::Addr) {
            match self.io.take() {
                Some(io) => (io, self.addr),
                None => std::future::pending().await,
            }
        }

        fn local_addr(&self) -> std::io::Result<Self::Addr> {
            Ok(self.addr)
        }
    }

    fn setup_state() -> Arc<FirstRunState> {
        Arc::new(FirstRunState {
            options: FirstRunServerOptions {
                output_dir: std::env::temp_dir().join(uuid::Uuid::new_v4().to_string()),
                control_group: "auction".to_owned(),
                control_uid: 1000,
            },
            token: "fixture-token".to_owned(),
            policy: Arc::new(FirstRunPolicy::default()),
            operation: Mutex::new(SetupOperation { status: SetupStatus::Pending, shutdown: None }),
        })
    }

    async fn setup_response(
        state: Arc<FirstRunState>,
        web_script_policy: WebScriptPolicy,
        host: &str,
    ) -> String {
        let request = format!(
            "POST /api/v1/setup/verify HTTP/1.1\r\nhost: {host}\r\nx-setup-token: fixture-token\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
        );
        raw_setup_response::<NoAssets>(state, web_script_policy, &request).await
    }

    async fn raw_setup_response<A: RustEmbed + 'static>(
        state: Arc<FirstRunState>,
        web_script_policy: WebScriptPolicy,
        request: &str,
    ) -> String {
        let app = first_run_router::<A>(state, web_script_policy);
        let (mut client, server) = duplex(4096);
        let listener =
            MemoryListener { io: Some(server), addr: SocketAddr::from(([127, 0, 0, 1], 12345)) }
                .tap_io(|_io| {});
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
                .await;
        });
        client.write_all(request.as_bytes()).await.expect("setup request write");

        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), client.read_to_end(&mut response))
            .await
            .expect("setup response timeout")
            .expect("setup response read");
        server.abort();
        String::from_utf8(response).expect("utf-8 setup response")
    }

    fn response_status(response: &str) -> u16 {
        response
            .split_whitespace()
            .nth(1)
            .and_then(|status| status.parse().ok())
            .expect("http status")
    }

    fn response_header<'a>(response: &'a str, name: &str) -> Option<&'a str> {
        response.lines().skip(1).take_while(|line| !line.is_empty()).find_map(|line| {
            let (header, value) = line.split_once(':')?;
            header.eq_ignore_ascii_case(name).then(|| value.trim())
        })
    }

    #[tokio::test]
    async fn setup_router_uses_the_selected_script_policy_for_allowed_and_rejected_requests() {
        for (web_script_policy, expected) in
            [(WebScriptPolicy::Dioxus, DIOXUS_CSP), (WebScriptPolicy::Solid, SOLID_CSP)]
        {
            let allowed = setup_response(setup_state(), web_script_policy, "127.0.0.1:8091").await;
            assert_eq!(response_status(&allowed), StatusCode::OK.as_u16(), "{web_script_policy:?}");
            assert_eq!(
                response_header(&allowed, "content-security-policy"),
                Some(expected),
                "{web_script_policy:?}"
            );
            assert_eq!(
                response_header(&allowed, "cache-control"),
                Some("no-store"),
                "{web_script_policy:?}"
            );

            let rejected =
                setup_response(setup_state(), web_script_policy, "evil.example:8091").await;
            assert_eq!(
                response_status(&rejected),
                StatusCode::FORBIDDEN.as_u16(),
                "{web_script_policy:?}"
            );
            assert_eq!(
                response_header(&rejected, "content-security-policy"),
                Some(expected),
                "{web_script_policy:?}"
            );
            assert_eq!(
                response_header(&rejected, "cache-control"),
                Some("no-store"),
                "{web_script_policy:?}"
            );
        }
    }

    #[tokio::test]
    async fn setup_router_serves_the_solid_entry_with_its_security_and_preload_headers() {
        let response = raw_setup_response::<SolidSetupAssets>(
            setup_state(),
            WebScriptPolicy::Solid,
            SETUP_PAGE_REQUEST,
        )
        .await;
        assert_eq!(response_status(&response), StatusCode::OK.as_u16());
        let (_, body) = response.split_once("\r\n\r\n").expect("设置页响应必须包含正文");
        assert_eq!(body.as_bytes(), SOLID_INDEX);
        assert_eq!(response_header(&response, "content-security-policy"), Some(SOLID_CSP));
        assert_eq!(response_header(&response, "cache-control"), Some("no-store"));
        assert_eq!(
            response_header(&response, "link"),
            Some(concat!(
                "</assets/index-B2UikXgQ.js>; rel=modulepreload; crossorigin, ",
                "</assets/rolldown-runtime-DK3Fl9T5.js>; rel=modulepreload; crossorigin, ",
                "</assets/index-ZX7NKpK-.css>; rel=preload; as=style; crossorigin",
            ))
        );
    }

    #[tokio::test]
    async fn setup_router_disables_asset_caching_and_keeps_dioxus_preload_semantics() {
        let asset_request = "GET /assets/index-B2UikXgQ.js HTTP/1.1\r\nhost: 127.0.0.1:8091\r\nconnection: close\r\n\r\n";
        let asset = raw_setup_response::<SolidSetupAssets>(
            setup_state(),
            WebScriptPolicy::Solid,
            asset_request,
        )
        .await;
        assert_eq!(response_status(&asset), StatusCode::OK.as_u16());
        for header in ["cache-control", "cdn-cache-control", "cloudflare-cdn-cache-control"] {
            assert_eq!(response_header(&asset, header), Some("no-store"), "{header}");
        }

        let dioxus = raw_setup_response::<SolidSetupAssets>(
            setup_state(),
            WebScriptPolicy::Dioxus,
            SETUP_PAGE_REQUEST,
        )
        .await;
        assert_eq!(response_status(&dioxus), StatusCode::OK.as_u16());
        // Dioxus 策略只转写 `<link>`，同一份入口文档据此只能得到样式表预加载。
        assert_eq!(
            response_header(&dioxus, "link"),
            Some("</assets/index-ZX7NKpK-.css>; rel=preload; as=style; crossorigin")
        );
    }

    #[tokio::test]
    async fn setup_asset_fallback_fingerprints_follow_the_selected_policy() {
        for (policy, expected) in [
            (WebScriptPolicy::Solid, "public, max-age=31536000, immutable"),
            (WebScriptPolicy::Dioxus, "no-cache"),
        ] {
            let request = Request::builder()
                .uri("/assets/index-B2UikXgQ.js")
                .body(Body::empty())
                .expect("setup asset request");
            let response =
                auction_http::serve_admin_setup_asset::<SolidSetupAssets>(request, policy).await;
            assert_eq!(response.status(), StatusCode::OK, "{policy:?}");
            assert_eq!(
                response.headers()[axum::http::header::CACHE_CONTROL],
                expected,
                "{policy:?}"
            );
        }
    }

    fn form() -> SetupRequest {
        SetupRequest {
            database: SetupDatabaseRequest {
                host: "::1".to_owned(),
                port: 5432,
                name: "拍卖 & %".to_owned(),
                username: "auction_runtime".to_owned(),
                password: "fixture-password".to_owned(),
            },
            admin_actor: "admin".to_owned(),
            // 在配置层停下，让匹配用例无需真实数据库且能区分是否已通过确认门禁。
            admin_secret: String::new(),
            public_origin: "https://public.example.test".to_owned(),
            admin_origin: "https://admin.example.test".to_owned(),
            public_listener_port: 8090,
            admin_listener_port: 8091,
            metrics_enabled: true,
            schedule_enabled: false,
            schedule: SetupScheduleRequest::default(),
            start_price: 1000,
            log_filter: "info".to_owned(),
            worker_concurrency: 4,
            worker_poll_interval_seconds: 15,
            realtime_poll_interval_millis: 250,
            confirm_initialize: true,
        }
    }

    fn test_state(initialize_and_start: bool) -> Arc<FirstRunState> {
        Arc::new(FirstRunState {
            options: FirstRunServerOptions {
                output_dir: std::env::temp_dir().join(uuid::Uuid::new_v4().to_string()),
                control_group: "auction".to_owned(),
                control_uid: 1000,
            },
            token: "fixture-token".to_owned(),
            policy: Arc::new(FirstRunPolicy { initialize_and_start, ..FirstRunPolicy::default() }),
            operation: Mutex::new(SetupOperation { status: SetupStatus::Pending, shutdown: None }),
        })
    }

    async fn completion_rejection(
        initialize_and_start: bool,
        form: SetupRequest,
        confirmations: &[&str],
    ) -> String {
        let state = test_state(initialize_and_start);
        let (parts, body) = Json(form).into_response().into_parts();
        let mut request = Request::new(body);
        *request.headers_mut() = parts.headers;
        request.headers_mut().insert("x-setup-token", "fixture-token".parse().unwrap());
        for confirmation in confirmations {
            request.headers_mut().append("x-setup-confirm-target", confirmation.parse().unwrap());
        }
        let response = complete(State(state.clone()), request).await.err().unwrap().into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(!state.options.output_dir.exists());
        let body = to_bytes(response.into_body(), 128).await.unwrap();
        String::from_utf8(body.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn setup_complete_rejects_missing_duplicate_and_changed_database_target() {
        for headers in [&[][..], &[DATABASE_TARGET, DATABASE_TARGET][..], &["bad header"][..]] {
            assert_eq!(
                completion_rejection(true, form(), headers).await,
                r#"{"code":"setup_confirmation_required"}"#
            );
        }
        for (host, port, name) in
            [("127.0.0.1", 5432, "拍卖 & %"), ("::1", 5433, "拍卖 & %"), ("::1", 5432, "other")]
        {
            let mut form = form();
            form.database.host = host.to_owned();
            form.database.port = port;
            form.database.name = name.to_owned();
            assert_eq!(
                completion_rejection(true, form, &[DATABASE_TARGET]).await,
                TARGET_MISMATCH_RESPONSE.trim_end()
            );
        }
        let mut form = form();
        form.confirm_initialize = false;
        assert_eq!(
            completion_rejection(true, form, &[DATABASE_TARGET]).await,
            r#"{"code":"setup_confirmation_required"}"#
        );
    }

    #[tokio::test]
    async fn setup_complete_checks_configuration_after_matching_database_confirmation() {
        for (initialize, headers) in [(true, &[DATABASE_TARGET][..]), (false, &[][..])] {
            assert_eq!(
                completion_rejection(initialize, form(), headers).await,
                r#"{"code":"invalid_input"}"#
            );
        }
    }

    #[tokio::test]
    async fn setup_validation_contract_fixture() {
        let state = test_state(false);
        let (parts, body) = Json(form()).into_response().into_parts();
        let mut request = Request::new(body);
        *request.headers_mut() = parts.headers;
        request.headers_mut().insert("x-setup-token", "fixture-token".parse().unwrap());
        let response =
            super::validate(State(state.clone()), request).await.err().unwrap().into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(!state.options.output_dir.exists());
        let body = to_bytes(response.into_body(), 128).await.unwrap();
        let expected = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../auction-contracts/tests/fixtures/admin/setup-validate-invalid.json"
        ));
        assert_eq!(body.as_ref(), expected.trim_end().as_bytes());
    }

    #[tokio::test]
    async fn setup_verify_contract_fixtures() {
        for (initialize, body) in [
            (
                false,
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../auction-contracts/tests/fixtures/admin/setup-verify.json"
                )),
            ),
            (
                true,
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../auction-contracts/tests/fixtures/admin/setup-verify-initialize.json"
                )),
            ),
        ] {
            let state = test_state(initialize);
            let mut headers = axum::http::HeaderMap::new();
            headers.insert("x-setup-token", "fixture-token".parse().unwrap());
            let response = super::verify(State(state.clone()), headers).await.into_response();
            assert_eq!(response.status(), StatusCode::OK);
            let actual = to_bytes(response.into_body(), 128).await.unwrap();
            assert_eq!(actual.as_ref(), body.trim_end().as_bytes());
            assert!(!state.options.output_dir.exists());
        }
    }
}
