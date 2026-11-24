//! 初始化页面的令牌门禁、数据库连通性检查与单次提交。

use std::{
    fs,
    io::{self, ErrorKind, Write},
    net::IpAddr,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};

use auction_contracts::{SetupErrorResponse, SetupResponse};
use auction_http::WebScriptPolicy;
use auction_storage::postgres_connect_options;
use axum::{Json, Router, extract::DefaultBodyLimit, http::StatusCode, routing::post};
use rust_embed::RustEmbed;
use tokio::{
    net::TcpListener,
    sync::{Mutex, oneshot},
};
use uuid::Uuid;

mod handlers;
use self::handlers::{complete, validate, verify};
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
    token_file: PathBuf,
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

fn create_setup_token_file(output_dir: &Path, token: &str) -> Result<PathBuf, &'static str> {
    if !output_dir.is_absolute() {
        return Err("setup output directory must be absolute");
    }
    let parent = output_dir.parent().ok_or("cannot locate setup output parent")?;
    let parent_metadata =
        fs::symlink_metadata(parent).map_err(|_| "cannot inspect setup output parent")?;
    // SAFETY: geteuid only reads the effective UID of this setup process.
    let effective_uid = unsafe { libc::geteuid() };
    if !parent_metadata.file_type().is_dir()
        || parent_metadata.mode() & 0o022 != 0
        || (parent_metadata.uid() != effective_uid && parent_metadata.uid() != 0)
    {
        return Err("setup output parent is not private to the setup operator");
    }

    let directory = parent.join(format!(".auction-setup-token-{}", Uuid::new_v4().simple()));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .map_err(|_| "cannot create private setup token directory")?;
    let token_file = directory.join("token");
    let result = (|| {
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .map_err(|_| "cannot secure setup token directory")?;
        let directory_metadata =
            fs::symlink_metadata(&directory).map_err(|_| "cannot verify setup token directory")?;
        if !directory_metadata.file_type().is_dir()
            || directory_metadata.uid() != effective_uid
            || directory_metadata.mode() & 0o7777 != 0o700
        {
            return Err("setup token directory has unsafe ownership or permissions");
        }

        let mut options = fs::OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let mut file = options.open(&token_file).map_err(|_| "cannot create setup token file")?;
        let metadata = file.metadata().map_err(|_| "cannot verify setup token file")?;
        if !metadata.file_type().is_file()
            || metadata.uid() != effective_uid
            || metadata.mode() & 0o7777 & !0o600 != 0
        {
            return Err("setup token file has unsafe ownership or permissions");
        }
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| "cannot secure setup token file")?;
        let metadata = file.metadata().map_err(|_| "cannot verify setup token file")?;
        if !metadata.file_type().is_file()
            || metadata.uid() != effective_uid
            || metadata.mode() & 0o7777 != 0o600
        {
            return Err("setup token file has unsafe ownership or permissions");
        }
        file.write_all(token.as_bytes()).map_err(|_| "cannot write setup token file")?;
        file.sync_all().map_err(|_| "cannot persist setup token file")?;
        fs::File::open(&directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| "cannot persist setup token directory")?;
        fs::File::open(parent)
            .and_then(|parent| parent.sync_all())
            .map_err(|_| "cannot persist setup token directory entry")?;
        Ok(token_file.clone())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&token_file);
        let _ = fs::remove_dir(&directory);
    }
    result
}

fn remove_setup_token(token_file: &Path) -> io::Result<()> {
    let directory = token_file
        .parent()
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "setup token file has no parent"))?;
    let parent = directory.parent().ok_or_else(|| {
        io::Error::new(ErrorKind::InvalidInput, "setup token directory has no parent")
    })?;
    // SAFETY: geteuid only reads the effective UID of this setup process.
    let effective_uid = unsafe { libc::geteuid() };
    let directory_metadata = fs::symlink_metadata(directory)?;
    let file_metadata = fs::symlink_metadata(token_file)?;
    if !directory_metadata.file_type().is_dir()
        || directory_metadata.uid() != effective_uid
        || directory_metadata.mode() & 0o7777 != 0o700
        || !file_metadata.file_type().is_file()
        || file_metadata.uid() != effective_uid
        || file_metadata.mode() & 0o7777 != 0o600
    {
        return Err(io::Error::new(ErrorKind::PermissionDenied, "setup token path is unsafe"));
    }
    fs::remove_file(token_file)?;
    fs::remove_dir(directory)?;
    fs::File::open(parent)?.sync_all()
}

fn write_setup_banner(writer: &mut impl Write, origin: &str, token_file: &Path) -> io::Result<()> {
    writeln!(writer, "首次部署页面：{origin}/setup")?;
    writeln!(writer, "一次性设置令牌文件：{}", token_file.display())?;
    writeln!(writer, "仅使用 SSH 隧道或已显式授权的 HTTPS 代理；不要公开明文初始化监听器。")
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
    let token_file = create_setup_token_file(&options.output_dir, &token)?;
    let (shutdown_sender, shutdown_receiver) = oneshot::channel();
    let state = Arc::new(FirstRunState {
        options,
        token,
        token_file,
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
    let mut stderr = io::stderr().lock();
    write_setup_banner(&mut stderr, &origin, &state.token_file)
        .map_err(|_| "cannot write setup token file path")?;

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

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::{borrow::Cow, fs, net::SocketAddr, sync::Arc, time::Duration};

    use auction_contracts::{
        ApiErrorResponse, SetupDatabaseRequest, SetupRequest, SetupScheduleRequest,
    };
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
        complete, create_setup_token_file, first_run_router, remove_setup_token,
        write_setup_banner,
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

    #[test]
    fn setup_token_is_private_and_logs_only_its_path() {
        let parent = std::env::temp_dir()
            .join(format!("auction-setup-token-test-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir(&parent).expect("private test parent");
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))
            .expect("secure test parent");
        let output_dir = parent.join("config");
        let secret = "one-time-fixture-secret";
        let token_file = create_setup_token_file(&output_dir, secret).expect("secure token file");
        let token_directory = token_file.parent().expect("token directory");
        // SAFETY: geteuid only reads the effective UID for these filesystem assertions.
        let effective_uid = unsafe { libc::geteuid() };
        let directory_metadata = fs::symlink_metadata(token_directory).expect("directory metadata");
        let file_metadata = fs::symlink_metadata(&token_file).expect("file metadata");
        assert!(directory_metadata.file_type().is_dir());
        assert_eq!(directory_metadata.uid(), effective_uid);
        assert_eq!(directory_metadata.mode() & 0o7777, 0o700);
        assert!(file_metadata.file_type().is_file());
        assert_eq!(file_metadata.uid(), effective_uid);
        assert_eq!(file_metadata.mode() & 0o7777, 0o600);
        assert_eq!(fs::read_to_string(&token_file).expect("token contents"), secret);

        let mut log = Vec::new();
        write_setup_banner(&mut log, "http://127.0.0.1:8091", &token_file).expect("banner");
        let log = String::from_utf8(log).expect("UTF-8 banner");
        assert!(log.contains(token_file.to_str().expect("absolute UTF-8 path")));
        assert!(!log.contains(secret));

        remove_setup_token(&token_file).expect("remove consumed token");
        assert!(!token_file.exists());
        assert!(!token_directory.exists());
        fs::remove_dir(parent).expect("remove test parent");
    }

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
            token_file: std::env::temp_dir().join("auction-setup-fixture-token/token"),
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
    async fn setup_router_excludes_formal_admin_consignment() {
        let state = setup_state();
        for (method, path, api) in [
            ("GET", "/consignment", false),
            ("GET", "/consignment/listings/00000000-0000-4000-8000-000000000008", false),
            ("GET", "/api/v1/admin/consignment/listings", true),
            (
                "GET",
                "/api/v1/admin/consignment/listings/00000000-0000-4000-8000-000000000008",
                true,
            ),
            (
                "POST",
                "/api/v1/admin/consignment/listings/00000000-0000-4000-8000-000000000008/delist",
                true,
            ),
        ] {
            let request = format!(
                "{method} {path} HTTP/1.1\r\nhost: 127.0.0.1:8091\r\naccept: text/html\r\nconnection: close\r\n\r\n"
            );
            let response = raw_setup_response::<SolidSetupAssets>(
                Arc::clone(&state),
                WebScriptPolicy::Solid,
                &request,
            )
            .await;
            assert_eq!(
                response_status(&response),
                StatusCode::NOT_FOUND.as_u16(),
                "{method} {path}"
            );
            if api {
                assert_eq!(
                    response_header(&response, "content-type"),
                    Some("application/json"),
                    "{path}"
                );
                let (_, body) = response.split_once("\r\n\r\n").expect("API 错误正文");
                let Json(error) =
                    Json::<ApiErrorResponse>::from_bytes(body.as_bytes()).expect("API 错误外壳");
                assert_eq!(error.code, "not_found", "{path}");
                assert_eq!(error.data, None, "{path}");
            } else {
                assert_eq!(
                    response_header(&response, "content-type"),
                    Some("text/html; charset=utf-8"),
                    "{path}"
                );
            }
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
            token_file: std::env::temp_dir().join("auction-setup-fixture-token/token"),

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
