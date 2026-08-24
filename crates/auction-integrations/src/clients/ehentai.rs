//! E-Hentai 论坛固定 origin 网关。
//!
//! 该适配器只实现旧论坛协议所需的两类操作：按标题查找或创建主题，以及读取已发布主题。
//! 所有响应都有硬字节上限；创建主题在 POST 发出后只接受可证明的远端主题身份，否则返回
//! `write_unknown`，上层必须先对账，不能盲目重发。

use std::{
    collections::HashSet,
    fmt,
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant},
};

use auction_application::{
    AdminCredentialRuntimeWriter, AdminForumPreviewResult, ApplicationError, ApplicationFuture,
    ForumPublishedThread, ForumPublisher, ForumRemoteError, ForumRenderedThread, ForumTopicReader,
};
use auction_domain::SettlementWorld;
use chrono::Utc;
use reqwest::{
    Client, Response, StatusCode,
    header::{self, HeaderValue},
    redirect::Policy,
};
use scraper::{Html, Selector};
use tokio::sync::Mutex;
use url::{Url, form_urlencoded};
use uuid::Uuid;

const DEFAULT_MAXIMUM_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const HARD_MAXIMUM_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAXIMUM_FORUM_BODY_BYTES: usize = 65_531;
const MAXIMUM_FORUM_TITLE_RUNES: usize = 50;
const MAXIMUM_FORM_FIELDS: usize = 128;
const MAXIMUM_FORM_FIELD_BYTES: usize = 4 * 1024;
const MAXIMUM_COOKIE_BYTES: usize = 16 * 1024;
const MAXIMUM_CREDENTIAL_CANDIDATES: usize = 32;
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_MINIMUM_REQUEST_INTERVAL: Duration = Duration::from_millis(750);
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";

/// 旧论坛生产入口。
pub const PRODUCTION_ENDPOINT: &str = "https://forums.e-hentai.org";
/// 旧版测试论坛板块。
pub const TEST_BOARD_ID: i32 = 4;
/// 旧版主世界拍卖板块。
pub const MAIN_BOARD_ID: i32 = 77;
/// 旧版异世界拍卖板块。
pub const ISEKAI_BOARD_ID: i32 = 90;

/// 论坛操作名称，用于安全错误分类和日志字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EhentaiOperation {
    CreateTopic,
    FetchTopic,
}

impl EhentaiOperation {
    /// 返回持久化和观测使用的稳定名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CreateTopic => "create_topic",
            Self::FetchTopic => "fetch_topic",
        }
    }
}

/// 论坛适配器对外暴露的稳定错误分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EhentaiErrorCategory {
    Configuration,
    CredentialUnavailable,
    SessionExpired,
    RateLimited,
    RemoteTemporary,
    ResponseTooLarge,
    ProtocolDrift,
    WriteUnknown,
}

impl EhentaiErrorCategory {
    /// 返回不包含远端正文和凭据的稳定错误码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Configuration => "configuration",
            Self::CredentialUnavailable => "credential_unavailable",
            Self::SessionExpired => "session_expired",
            Self::RateLimited => "rate_limited",
            Self::RemoteTemporary => "remote_temporary",
            Self::ResponseTooLarge => "response_too_large",
            Self::ProtocolDrift => "protocol_drift",
            Self::WriteUnknown => "write_unknown",
        }
    }
}

/// E-Hentai 适配器错误。
///
/// 该错误故意不保存 URL、Cookie、标题、帖子正文或远端响应，避免错误被日志、任务状态或
/// 命令结果意外带出敏感数据。`write_started` 是创建主题的重试栅栏：一旦为真，凭据不能
/// 通过错误自动轮换，且任务必须先做远端对账。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EhentaiError {
    operation: EhentaiOperation,
    category: EhentaiErrorCategory,
    retryable: bool,
    write_started: bool,
}

impl EhentaiError {
    /// 返回操作类别。
    pub const fn operation(self) -> EhentaiOperation {
        self.operation
    }

    /// 返回稳定错误分类。
    pub const fn category(self) -> EhentaiErrorCategory {
        self.category
    }

    /// 返回该错误是否可以由 Worker 退避后再次处理。
    pub const fn retryable(self) -> bool {
        self.retryable
    }

    /// 返回请求是否已经进入远端写入阶段。
    pub const fn write_started(self) -> bool {
        self.write_started
    }

    /// 只有读侧、尚未发生写入时才允许凭据轮换。
    pub const fn is_session_expired(self) -> bool {
        matches!(self.category, EhentaiErrorCategory::SessionExpired) && !self.write_started
    }

    /// 转换为应用层安全错误，不携带远端响应或凭据内容。
    pub const fn into_application_error(self) -> ApplicationError {
        ApplicationError::ForumRemote(ForumRemoteError::new(
            self.category.as_str(),
            self.retryable,
            self.write_started,
        ))
    }

    const fn new(
        operation: EhentaiOperation,
        category: EhentaiErrorCategory,
        retryable: bool,
        write_started: bool,
    ) -> Self {
        Self { operation, category, retryable, write_started }
    }

    const fn configuration(operation: EhentaiOperation) -> Self {
        Self::new(operation, EhentaiErrorCategory::Configuration, false, false)
    }

    const fn credential(operation: EhentaiOperation) -> Self {
        Self::new(operation, EhentaiErrorCategory::CredentialUnavailable, false, false)
    }

    const fn session_expired(operation: EhentaiOperation) -> Self {
        Self::new(operation, EhentaiErrorCategory::SessionExpired, false, false)
    }

    const fn rate_limited(operation: EhentaiOperation, write_started: bool) -> Self {
        Self::new(operation, EhentaiErrorCategory::RateLimited, !write_started, write_started)
    }

    const fn remote_temporary(operation: EhentaiOperation, write_started: bool) -> Self {
        Self::new(operation, EhentaiErrorCategory::RemoteTemporary, !write_started, write_started)
    }

    const fn response_too_large(operation: EhentaiOperation) -> Self {
        Self::new(operation, EhentaiErrorCategory::ResponseTooLarge, false, false)
    }

    const fn protocol(operation: EhentaiOperation, write_started: bool) -> Self {
        Self::new(operation, EhentaiErrorCategory::ProtocolDrift, false, write_started)
    }

    const fn write_unknown(operation: EhentaiOperation) -> Self {
        Self::new(operation, EhentaiErrorCategory::WriteUnknown, true, true)
    }
}

impl fmt::Display for EhentaiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "E-Hentai forum operation failed: operation={} category={} retryable={} write_started={}",
            self.operation.as_str(),
            self.category.as_str(),
            self.retryable,
            self.write_started,
        )
    }
}

impl std::error::Error for EhentaiError {}

/// 经过边界校验的 E-Hentai Cookie。
#[derive(Clone, PartialEq, Eq)]
pub struct EhentaiCookie(String);

impl EhentaiCookie {
    /// 从密钥管理器刚解出的字节建立 Cookie，并拒绝 header 注入字符。
    pub fn from_bytes(value: &[u8]) -> Result<Self, EhentaiError> {
        if value.is_empty()
            || value.len() > MAXIMUM_COOKIE_BYTES
            || value.iter().any(|byte| !(0x20..=0x7e).contains(byte))
        {
            return Err(EhentaiError::credential(EhentaiOperation::CreateTopic));
        }
        let value = String::from_utf8(value.to_vec())
            .map_err(|_| EhentaiError::credential(EhentaiOperation::CreateTopic))?;
        Ok(Self(value))
    }

    /// 返回仅供建立单次 HTTP header 使用的 Cookie 文本。
    fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for EhentaiCookie {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EhentaiCookie(REDACTED)")
    }
}

#[derive(Clone)]
struct EhentaiCredentialCandidate {
    key: Uuid,
    persistent_id: Option<Uuid>,
    cookie: EhentaiCookie,
}

/// 一组按稳定顺序尝试的论坛凭据。
#[derive(Clone)]
pub struct EhentaiCredentialSet {
    candidates: Arc<StdMutex<Vec<EhentaiCredentialCandidate>>>,
    invalidator: Option<Arc<dyn AdminCredentialRuntimeWriter>>,
}

impl EhentaiCredentialSet {
    /// 从解密后的 Cookie 建立凭据候选集。
    pub fn from_cookies(cookies: Vec<EhentaiCookie>) -> Result<Self, EhentaiError> {
        Self::from_candidates(
            cookies
                .into_iter()
                .map(|cookie| EhentaiCredentialCandidate {
                    key: Uuid::new_v4(),
                    persistent_id: None,
                    cookie,
                })
                .collect(),
            None,
            false,
        )
    }

    /// 从数据库解密后的 Cookie 建立可回写失效状态的集合。
    pub fn from_runtime_cookies(
        cookies: Vec<(Uuid, EhentaiCookie)>,
        invalidator: Arc<dyn AdminCredentialRuntimeWriter>,
    ) -> Result<Self, EhentaiError> {
        Self::from_candidates(
            cookies
                .into_iter()
                .map(|(persistent_id, cookie)| EhentaiCredentialCandidate {
                    key: Uuid::new_v4(),
                    persistent_id: Some(persistent_id),
                    cookie,
                })
                .collect(),
            Some(invalidator),
            true,
        )
    }

    fn from_candidates(
        candidates: Vec<EhentaiCredentialCandidate>,
        invalidator: Option<Arc<dyn AdminCredentialRuntimeWriter>>,
        allow_empty: bool,
    ) -> Result<Self, EhentaiError> {
        if (!allow_empty && candidates.is_empty())
            || candidates.len() > 64
            || candidates
                .iter()
                .any(|candidate| candidate.persistent_id.is_some_and(|id| id.is_nil()))
            || candidates.iter().any(|candidate| candidate.persistent_id.is_some())
                && invalidator.is_none()
        {
            return Err(EhentaiError::credential(EhentaiOperation::CreateTopic));
        }
        Ok(Self { candidates: Arc::new(StdMutex::new(candidates)), invalidator })
    }

    /// 从一至两个本地密钥明文建立凭据候选集。
    pub fn from_secrets(first: &[u8], second: Option<&[u8]>) -> Result<Self, EhentaiError> {
        let mut cookies = vec![EhentaiCookie::from_bytes(first)?];
        if let Some(second) = second {
            cookies.push(EhentaiCookie::from_bytes(second)?);
        }
        Self::from_cookies(cookies)
    }

    /// 返回候选集是否为空。
    pub fn is_empty(&self) -> bool {
        self.candidates.lock().map(|candidates| candidates.is_empty()).unwrap_or(true)
    }

    fn begin(&self) -> Result<EhentaiCredentialSession, EhentaiError> {
        let candidate = self
            .candidates
            .lock()
            .map_err(|_| EhentaiError::credential(EhentaiOperation::FetchTopic))?
            .first()
            .cloned()
            .ok_or_else(|| EhentaiError::credential(EhentaiOperation::FetchTopic))?;
        Ok(EhentaiCredentialSession {
            set: self.clone(),
            current: candidate.clone(),
            tried: HashSet::from([candidate.key]),
        })
    }
}

impl fmt::Debug for EhentaiCredentialSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EhentaiCredentialSet")
            .field(
                "candidate_count",
                &self.candidates.lock().map(|candidates| candidates.len()).unwrap_or(0),
            )
            .finish()
    }
}

/// 一次论坛逻辑操作的稳定凭据选择。
struct EhentaiCredentialSession {
    set: EhentaiCredentialSet,
    current: EhentaiCredentialCandidate,
    tried: HashSet<Uuid>,
}

impl EhentaiCredentialSession {
    fn cookie(&self) -> &EhentaiCookie {
        &self.current.cookie
    }

    async fn invalidate_current(&mut self) -> Result<(), EhentaiError> {
        let current = self.current.clone();
        {
            let mut candidates = self
                .set
                .candidates
                .lock()
                .map_err(|_| EhentaiError::credential(EhentaiOperation::FetchTopic))?;
            candidates.retain(|candidate| candidate.key != current.key);
        }
        if let (Some(persistent_id), Some(invalidator)) =
            (current.persistent_id, self.set.invalidator.as_ref())
        {
            invalidator
                .mark_invalid(persistent_id, Utc::now())
                .await
                .map_err(|_| EhentaiError::credential(EhentaiOperation::FetchTopic))?;
        }
        Ok(())
    }

    fn advance(&mut self) -> Result<(), EhentaiError> {
        let candidate = self
            .set
            .candidates
            .lock()
            .map_err(|_| EhentaiError::credential(EhentaiOperation::FetchTopic))?
            .iter()
            .find(|candidate| !self.tried.contains(&candidate.key))
            .cloned()
            .ok_or_else(|| EhentaiError::credential(EhentaiOperation::FetchTopic))?;
        self.tried.insert(candidate.key);
        self.current = candidate;
        Ok(())
    }
}

/// E-Hentai HTTP 客户端配置。
#[derive(Debug, Clone)]
pub struct EhentaiClientConfig {
    /// 固定论坛 origin；生产环境必须使用 HTTPS。
    pub endpoint: Url,
    /// 单次响应最大字节数，不能超过 8 MiB。
    pub maximum_response_bytes: usize,
    /// 连续请求之间的最小间隔。
    pub minimum_request_interval: Duration,
    /// 单次 HTTP 请求超时时间。
    pub request_timeout: Duration,
    /// 是否把所有世界映射到旧版测试板块 4。
    pub test_board_mode: bool,
}

impl EhentaiClientConfig {
    /// 返回生产固定 origin 的默认配置。
    pub fn production() -> Result<Self, EhentaiError> {
        let endpoint = Url::parse(PRODUCTION_ENDPOINT)
            .map_err(|_| EhentaiError::configuration(EhentaiOperation::CreateTopic))?;
        Ok(Self {
            endpoint,
            maximum_response_bytes: DEFAULT_MAXIMUM_RESPONSE_BYTES,
            minimum_request_interval: DEFAULT_MINIMUM_REQUEST_INTERVAL,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            test_board_mode: false,
        })
    }

    /// 校验固定 origin、资源上限和时间边界。
    fn validate(&self, operation: EhentaiOperation) -> Result<(), EhentaiError> {
        let valid_scheme = self.endpoint.scheme() == "https"
            || (self.endpoint.scheme() == "http" && is_loopback_host(&self.endpoint));
        if !valid_scheme
            || self.endpoint.host_str().is_none()
            || !self.endpoint.username().is_empty()
            || self.endpoint.password().is_some()
            || self.maximum_response_bytes == 0
            || self.maximum_response_bytes > HARD_MAXIMUM_RESPONSE_BYTES
            || self.minimum_request_interval.is_zero()
            || self.request_timeout.is_zero()
            || self.request_timeout > Duration::from_secs(300)
        {
            return Err(EhentaiError::configuration(operation));
        }
        Ok(())
    }
}

/// 受限的 E-Hentai HTTP 客户端。
#[derive(Clone)]
pub struct EhentaiClient {
    client: Client,
    base_url: Url,
    origin: String,
    gate: Arc<Mutex<Instant>>,
    maximum_response_bytes: usize,
    request_timeout: Duration,
    minimum_request_interval: Duration,
    test_board_mode: bool,
}

impl EhentaiClient {
    /// 使用已有 HTTP client 建立兼容构造器。
    ///
    /// 生产装配应使用 [`Self::with_config`]，它会强制关闭自动重定向；该方法保留给已有
    /// 装配代码和受控本地协议适配器使用。
    pub fn new(base_url: Url, client: Client) -> Self {
        let origin = origin_of(&base_url).unwrap_or_default();
        Self {
            client,
            base_url,
            origin,
            gate: Arc::new(Mutex::new(Instant::now())),
            maximum_response_bytes: DEFAULT_MAXIMUM_RESPONSE_BYTES,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            minimum_request_interval: DEFAULT_MINIMUM_REQUEST_INTERVAL,
            test_board_mode: false,
        }
    }

    /// 使用安全默认 HTTP client 建立客户端。
    pub fn with_config(config: EhentaiClientConfig) -> Result<Self, EhentaiError> {
        config.validate(EhentaiOperation::CreateTopic)?;
        let client = build_forum_http_client()
            .map_err(|_| EhentaiError::configuration(EhentaiOperation::CreateTopic))?;
        Self::from_config_and_client(config, client)
    }

    /// 使用调用方提供的 client 建立客户端；调用方必须保证其不跟随重定向。
    pub fn from_config_and_client(
        config: EhentaiClientConfig,
        client: Client,
    ) -> Result<Self, EhentaiError> {
        config.validate(EhentaiOperation::CreateTopic)?;
        let origin = origin_of(&config.endpoint)
            .ok_or(EhentaiError::configuration(EhentaiOperation::CreateTopic))?;
        Ok(Self {
            client,
            base_url: config.endpoint,
            origin,
            gate: Arc::new(Mutex::new(Instant::now())),
            maximum_response_bytes: config.maximum_response_bytes,
            request_timeout: config.request_timeout,
            minimum_request_interval: config.minimum_request_interval,
            test_board_mode: config.test_board_mode,
        })
    }

    /// 返回底层 HTTP client，供需要共享连接池的装配代码使用。
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// 返回固定论坛 origin。
    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    /// 创建或按精确标题查找一个论坛主题。
    pub async fn create_topic(
        &self,
        world: SettlementWorld,
        cookie: &EhentaiCookie,
        rendered: ForumRenderedThread,
    ) -> Result<auction_application::ForumPublishedThread, EhentaiError> {
        let operation = EhentaiOperation::CreateTopic;
        if !self.valid_for() || !valid_cookie(cookie) || !valid_rendered_thread(&rendered) {
            return Err(EhentaiError::configuration(operation));
        }
        let board_id = self.board_for_world(world, operation)?;
        let board_value = board_id.to_string();
        let board_url = self.index_url(&[("showforum", board_value.as_str())], operation)?;
        let (body, _) = self.get_page(operation, board_url.clone(), None, cookie).await?;
        if let Some(published) =
            self.find_topic(&board_url, board_id, &rendered.title, &body, operation)?
        {
            return Ok(published);
        }

        let form = self.load_topic_form(board_id, &board_url, cookie).await?;
        self.submit_topic(form, board_id, rendered, cookie).await
    }

    /// 读取一个已发布主题，并复用论坛预览适配器解析帖子命令。
    pub async fn fetch_topic(
        &self,
        world: SettlementWorld,
        thread_id: &str,
        cookie: &EhentaiCookie,
    ) -> Result<AdminForumPreviewResult, EhentaiError> {
        let operation = EhentaiOperation::FetchTopic;
        if !self.valid_for() || !valid_cookie(cookie) || !valid_topic_id(thread_id) {
            return Err(EhentaiError::configuration(operation));
        }
        let board_id = self.board_for_world(world, operation)?;
        let target = self.index_url(&[("showtopic", thread_id)], operation)?;
        let (body, _) = self.get_page(operation, target.clone(), Some(&target), cookie).await?;
        if board_id <= 0 {
            return Err(EhentaiError::configuration(operation));
        }
        let html =
            std::str::from_utf8(&body).map_err(|_| EhentaiError::protocol(operation, false))?;
        crate::ForumPreviewAdapter::new()
            .parse_html_document(html)
            .map_err(|_| EhentaiError::protocol(operation, false))
    }

    async fn get_page(
        &self,
        operation: EhentaiOperation,
        target: Url,
        referer: Option<&Url>,
        cookie: &EhentaiCookie,
    ) -> Result<(Vec<u8>, StatusCode), EhentaiError> {
        self.wait_rate().await;
        let mut request = self
            .client
            .get(target)
            .header(header::USER_AGENT, USER_AGENT)
            .header(header::COOKIE, cookie_header(cookie, operation)?);
        if let Some(referer) = referer {
            request = request.header(header::REFERER, referer.as_str());
        }
        let response = request
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(|_| EhentaiError::remote_temporary(operation, false))?;
        let status = response.status();
        let location = response.headers().get(header::LOCATION).cloned();
        let body = self.read_response(response, operation, false).await?;
        if is_session_expired(status, location.as_ref(), &body) {
            return Err(EhentaiError::session_expired(operation));
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(EhentaiError::rate_limited(operation, false));
        }
        if status.is_server_error() {
            return Err(EhentaiError::remote_temporary(operation, false));
        }
        if !status.is_success() {
            return Err(EhentaiError::protocol(operation, false));
        }
        if location.is_some() || !body_looks_like_html(&body) {
            return Err(EhentaiError::protocol(operation, false));
        }
        Ok((body, status))
    }

    async fn read_response(
        &self,
        mut response: Response,
        operation: EhentaiOperation,
        write_started: bool,
    ) -> Result<Vec<u8>, EhentaiError> {
        if response
            .content_length()
            .is_some_and(|length| length > self.maximum_response_bytes as u64)
        {
            return Err(if write_started {
                EhentaiError::write_unknown(operation)
            } else {
                EhentaiError::response_too_large(operation)
            });
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| {
            if write_started {
                EhentaiError::write_unknown(operation)
            } else {
                EhentaiError::remote_temporary(operation, false)
            }
        })? {
            if body.len().saturating_add(chunk.len()) > self.maximum_response_bytes {
                return Err(if write_started {
                    EhentaiError::write_unknown(operation)
                } else {
                    EhentaiError::response_too_large(operation)
                });
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    async fn load_topic_form(
        &self,
        board_id: i32,
        board_url: &Url,
        cookie: &EhentaiCookie,
    ) -> Result<TopicForm, EhentaiError> {
        let operation = EhentaiOperation::CreateTopic;
        let board_value = board_id.to_string();
        let target = self.index_url(
            &[("act", "Post"), ("CODE", "00"), ("f", board_value.as_str())],
            operation,
        )?;
        let (body, _) = self.get_page(operation, target.clone(), Some(board_url), cookie).await?;
        let html =
            std::str::from_utf8(&body).map_err(|_| EhentaiError::protocol(operation, false))?;
        let document = Html::parse_document(html);
        let form_selector =
            Selector::parse("form").map_err(|_| EhentaiError::protocol(operation, false))?;
        let input_selector = Selector::parse("input[type='hidden'][name]")
            .map_err(|_| EhentaiError::protocol(operation, false))?;
        let title_selector = Selector::parse("[name='TopicTitle']")
            .map_err(|_| EhentaiError::protocol(operation, false))?;
        let post_selector = Selector::parse("[name='Post']")
            .map_err(|_| EhentaiError::protocol(operation, false))?;
        for form in document.select(&form_selector) {
            let Some(values) = posting_form_values(
                form,
                &input_selector,
                &title_selector,
                &post_selector,
                board_id,
            ) else {
                continue;
            };
            let action_value = form.value().attr("action").unwrap_or(target.as_str());
            let action = self.resolve_same_origin(&target, action_value, operation)?;
            if !posting_action_matches_board(&action, board_id)
                || !form.value().attr("method").unwrap_or("GET").eq_ignore_ascii_case("POST")
            {
                continue;
            }
            return Ok(TopicForm { action, referer: target, values });
        }
        Err(EhentaiError::protocol(operation, false))
    }

    async fn submit_topic(
        &self,
        form: TopicForm,
        board_id: i32,
        rendered: ForumRenderedThread,
        cookie: &EhentaiCookie,
    ) -> Result<auction_application::ForumPublishedThread, EhentaiError> {
        let operation = EhentaiOperation::CreateTopic;
        let mut values = form.values;
        set_form_value(&mut values, "TopicTitle", rendered.title);
        set_form_value(&mut values, "TopicDesc", String::new());
        set_form_value(&mut values, "Post", rendered.body);
        set_form_value(&mut values, "f", board_id.to_string());
        set_form_value(&mut values, "REPLIER", "1".to_owned());
        set_form_value(&mut values, "bbmode", "normal".to_owned());
        set_form_value(&mut values, "enableemo", "yes".to_owned());
        if values.len() > MAXIMUM_FORM_FIELDS {
            return Err(EhentaiError::protocol(operation, false));
        }
        let encoded = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(values.iter().map(|(name, value)| (name.as_str(), value.as_str())))
            .finish();

        self.wait_rate().await;
        let response = self
            .client
            .post(form.action.clone())
            .header(header::USER_AGENT, USER_AGENT)
            .header(header::COOKIE, cookie_header(cookie, operation)?)
            .header(header::ORIGIN, self.origin.as_str())
            .header(header::REFERER, form.referer.as_str())
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .timeout(self.request_timeout)
            .body(encoded)
            .send()
            .await
            .map_err(|_| EhentaiError::write_unknown(operation))?;
        let status = response.status();
        let location = response.headers().get(header::LOCATION).cloned();
        let body = self.read_response(response, operation, true).await?;

        if is_session_expired(status, location.as_ref(), &body) {
            return Err(EhentaiError::new(
                operation,
                EhentaiErrorCategory::SessionExpired,
                false,
                true,
            ));
        }
        if status.is_redirection() {
            let Some(location) = location.and_then(|value| value.to_str().ok().map(str::to_owned))
            else {
                return Err(EhentaiError::write_unknown(operation));
            };
            return self
                .topic_from_reference(&form.action, &location, board_id, operation)
                .ok_or(EhentaiError::write_unknown(operation));
        }
        if status == StatusCode::OK
            && let Some(published) =
                self.topic_from_redirect_page(&form.action, &body, board_id, operation)
        {
            return Ok(published);
        }
        Err(EhentaiError::write_unknown(operation))
    }

    fn find_topic(
        &self,
        board_url: &Url,
        board_id: i32,
        title: &str,
        body: &[u8],
        operation: EhentaiOperation,
    ) -> Result<Option<auction_application::ForumPublishedThread>, EhentaiError> {
        let html =
            std::str::from_utf8(body).map_err(|_| EhentaiError::protocol(operation, false))?;
        let document = Html::parse_document(html);
        let selector =
            Selector::parse("a[href]").map_err(|_| EhentaiError::protocol(operation, false))?;
        let wanted = clean_line(title);
        for element in document.select(&selector) {
            let label = clean_line(&element.text().collect::<String>());
            if label != wanted {
                continue;
            }
            let Some(reference) = element.value().attr("href") else {
                continue;
            };
            if let Some(published) =
                self.topic_from_reference(board_url, reference, board_id, operation)
            {
                return Ok(Some(published));
            }
        }
        Ok(None)
    }

    fn topic_from_redirect_page(
        &self,
        base: &Url,
        body: &[u8],
        board_id: i32,
        operation: EhentaiOperation,
    ) -> Option<auction_application::ForumPublishedThread> {
        let html = std::str::from_utf8(body).ok()?;
        let document = Html::parse_document(html);
        let selector = Selector::parse("a[href]").ok()?;
        for element in document.select(&selector) {
            let Some(reference) = element.value().attr("href") else {
                continue;
            };
            if let Some(published) = self.topic_from_reference(base, reference, board_id, operation)
            {
                return Some(published);
            }
        }
        None
    }

    fn topic_from_reference(
        &self,
        base: &Url,
        reference: &str,
        board_id: i32,
        operation: EhentaiOperation,
    ) -> Option<auction_application::ForumPublishedThread> {
        let resolved = self.resolve_same_origin(base, reference, operation).ok()?;
        let mut topic_id = None;
        for (key, value) in resolved.query_pairs() {
            if key == "showtopic" || key == "t" {
                topic_id = Some(value.into_owned());
                break;
            }
        }
        let topic_id = topic_id?;
        if !valid_topic_id(&topic_id) || !posting_action_matches_board(&resolved, board_id) {
            return None;
        }
        let canonical = self.index_url(&[("showtopic", topic_id.as_str())], operation).ok()?;
        Some(auction_application::ForumPublishedThread {
            board_id,
            thread_id: topic_id,
            thread_url: canonical.to_string(),
        })
    }

    fn resolve_same_origin(
        &self,
        base: &Url,
        reference: &str,
        operation: EhentaiOperation,
    ) -> Result<Url, EhentaiError> {
        let resolved =
            base.join(reference).map_err(|_| EhentaiError::protocol(operation, false))?;
        if !same_origin(&resolved, &self.base_url)
            || !resolved.username().is_empty()
            || resolved.password().is_some()
        {
            return Err(EhentaiError::protocol(operation, false));
        }
        Ok(resolved)
    }

    fn index_url(
        &self,
        pairs: &[(&str, &str)],
        operation: EhentaiOperation,
    ) -> Result<Url, EhentaiError> {
        if self.origin.is_empty() {
            return Err(EhentaiError::configuration(operation));
        }
        let mut target = self.base_url.clone();
        target.set_path("/index.php");
        target.set_fragment(None);
        let query = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(pairs.iter().copied())
            .finish();
        target.set_query(Some(&query));
        Ok(target)
    }

    fn board_for_world(
        &self,
        world: SettlementWorld,
        _operation: EhentaiOperation,
    ) -> Result<i32, EhentaiError> {
        if self.test_board_mode {
            return Ok(TEST_BOARD_ID);
        }
        Ok(match world {
            SettlementWorld::Main => MAIN_BOARD_ID,
            SettlementWorld::Isekai => ISEKAI_BOARD_ID,
        })
    }

    fn valid_for(&self) -> bool {
        !self.origin.is_empty()
            && self.base_url.host_str().is_some()
            && (self.base_url.scheme() == "https"
                || (self.base_url.scheme() == "http" && is_loopback_host(&self.base_url)))
            && self.maximum_response_bytes > 0
            && self.maximum_response_bytes <= HARD_MAXIMUM_RESPONSE_BYTES
            && !self.minimum_request_interval.is_zero()
            && !self.request_timeout.is_zero()
    }

    async fn wait_rate(&self) {
        let wait = {
            let mut next = self.gate.lock().await;
            let now = Instant::now();
            let start = if *next > now { *next } else { now };
            let wait = if start > now { start - now } else { Duration::ZERO };
            *next = start.checked_add(self.minimum_request_interval).unwrap_or(start);
            wait
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
}

impl fmt::Debug for EhentaiClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EhentaiClient")
            .field("base_url", &self.base_url)
            .field("maximum_response_bytes", &self.maximum_response_bytes)
            .field("request_timeout", &self.request_timeout)
            .field("minimum_request_interval", &self.minimum_request_interval)
            .field("test_board_mode", &self.test_board_mode)
            .finish()
    }
}

/// 负责按读侧 session 过期结果轮换论坛凭据的网关。
#[derive(Clone)]
pub struct EhentaiGateway {
    client: Arc<EhentaiClient>,
    credentials: EhentaiCredentialSet,
}

impl EhentaiGateway {
    /// 建立不读取凭据、不发起网络请求的网关。
    pub fn new(
        client: Arc<EhentaiClient>,
        credentials: EhentaiCredentialSet,
    ) -> Result<Self, EhentaiError> {
        if credentials.is_empty() {
            return Err(EhentaiError::credential(EhentaiOperation::CreateTopic));
        }
        Ok(Self { client, credentials })
    }

    /// 创建主题；只有读侧 session 过期才会尝试下一个 Cookie。
    pub async fn create_topic(
        &self,
        world: SettlementWorld,
        rendered: ForumRenderedThread,
    ) -> Result<auction_application::ForumPublishedThread, EhentaiError> {
        let mut session = self.credentials.begin()?;
        for _ in 0..MAXIMUM_CREDENTIAL_CANDIDATES {
            match self.client.create_topic(world, session.cookie(), rendered.clone()).await {
                Ok(published) => return Ok(published),
                Err(error) if error.is_session_expired() => {
                    if session.invalidate_current().await.is_err() || session.advance().is_err() {
                        return Err(EhentaiError::credential(EhentaiOperation::CreateTopic));
                    }
                }
                Err(error) if error.category() == EhentaiErrorCategory::SessionExpired => {
                    let _ = session.invalidate_current().await;
                    return Err(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(EhentaiError::credential(EhentaiOperation::CreateTopic))
    }

    /// 读取主题；读取失败且确认为 session 过期时才会尝试下一个 Cookie。
    pub async fn fetch_topic(
        &self,
        world: SettlementWorld,
        thread_id: &str,
    ) -> Result<AdminForumPreviewResult, EhentaiError> {
        let mut session = self.credentials.begin()?;
        for _ in 0..MAXIMUM_CREDENTIAL_CANDIDATES {
            match self.client.fetch_topic(world, thread_id, session.cookie()).await {
                Ok(result) => return Ok(result),
                Err(error) if error.is_session_expired() => {
                    if session.invalidate_current().await.is_err() || session.advance().is_err() {
                        return Err(EhentaiError::credential(EhentaiOperation::FetchTopic));
                    }
                }
                Err(error) if error.category() == EhentaiErrorCategory::SessionExpired => {
                    let _ = session.invalidate_current().await;
                    return Err(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(EhentaiError::credential(EhentaiOperation::FetchTopic))
    }
}

impl fmt::Debug for EhentaiGateway {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EhentaiGateway")
            .field("client", &self.client)
            .field("credentials", &self.credentials)
            .finish()
    }
}

impl ForumPublisher for EhentaiGateway {
    fn create_topic(
        &self,
        world: SettlementWorld,
        rendered: ForumRenderedThread,
    ) -> ApplicationFuture<'_, ForumPublishedThread> {
        Box::pin(async move {
            EhentaiGateway::create_topic(self, world, rendered)
                .await
                .map_err(EhentaiError::into_application_error)
        })
    }
}

impl ForumTopicReader for EhentaiGateway {
    fn fetch_topic(
        &self,
        world: SettlementWorld,
        thread_id: String,
    ) -> ApplicationFuture<'_, AdminForumPreviewResult> {
        Box::pin(async move {
            EhentaiGateway::fetch_topic(self, world, &thread_id)
                .await
                .map_err(EhentaiError::into_application_error)
        })
    }
}

struct TopicForm {
    action: Url,
    referer: Url,
    values: Vec<(String, String)>,
}

fn posting_form_values(
    form: scraper::ElementRef<'_>,
    input_selector: &Selector,
    title_selector: &Selector,
    post_selector: &Selector,
    board_id: i32,
) -> Option<Vec<(String, String)>> {
    if form.select(title_selector).next().is_none() || form.select(post_selector).next().is_none() {
        return None;
    }
    let mut values = Vec::new();
    for input in form.select(input_selector) {
        let name = input.value().attr("name")?.to_owned();
        let value = input.value().attr("value").unwrap_or_default().to_owned();
        if !valid_form_field(&name, &value) || values.len() >= MAXIMUM_FORM_FIELDS {
            return None;
        }
        values.push((name, value));
    }
    let board = board_id.to_string();
    if value_for(&values, "act") != Some("Post")
        || value_for(&values, "CODE") != Some("01")
        || value_for(&values, "f") != Some(board.as_str())
        || value_for(&values, "auth_key").is_none_or(|value| value.is_empty())
    {
        return None;
    }
    Some(values)
}

fn posting_action_matches_board(action: &Url, board_id: i32) -> bool {
    let wanted = board_id.to_string();
    for key in ["f", "showforum"] {
        let values: Vec<_> = action.query_pairs().filter(|(name, _)| name == key).collect();
        if !values.is_empty() && (values.len() != 1 || values[0].1.as_ref() != wanted.as_str()) {
            return false;
        }
    }
    true
}

fn set_form_value(values: &mut Vec<(String, String)>, name: &str, value: String) {
    values.retain(|(current, _)| current != name);
    values.push((name.to_owned(), value));
}

fn value_for<'a>(values: &'a [(String, String)], name: &str) -> Option<&'a str> {
    values.iter().find(|(current, _)| current == name).map(|(_, value)| value.as_str())
}

fn cookie_header(
    cookie: &EhentaiCookie,
    operation: EhentaiOperation,
) -> Result<HeaderValue, EhentaiError> {
    HeaderValue::from_str(cookie.as_str()).map_err(|_| EhentaiError::credential(operation))
}

fn valid_cookie(cookie: &EhentaiCookie) -> bool {
    !cookie.0.is_empty() && cookie.0.len() <= MAXIMUM_COOKIE_BYTES
}

fn valid_rendered_thread(rendered: &ForumRenderedThread) -> bool {
    let title = rendered.title.trim();
    let body = rendered.body.trim();
    !title.is_empty()
        && title == rendered.title
        && rendered.title.chars().count() <= MAXIMUM_FORUM_TITLE_RUNES
        && rendered.title.chars().all(|character| !character.is_control())
        && !body.is_empty()
        && rendered.body.len() <= MAXIMUM_FORUM_BODY_BYTES
        && rendered
            .body
            .chars()
            .all(|character| !character.is_control() || matches!(character, '\n' | '\r' | '\t'))
}

fn valid_form_field(name: &str, value: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && value.len() <= MAXIMUM_FORM_FIELD_BYTES
        && name.is_ascii()
        && !name.chars().any(char::is_control)
        && !value.chars().any(|character| matches!(character, '\0' | '\r' | '\n'))
}

fn valid_topic_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.bytes().any(|byte| byte != b'0')
}

fn clean_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn body_looks_like_html(body: &[u8]) -> bool {
    let text = String::from_utf8_lossy(body).to_ascii_lowercase();
    text.contains("<html") || text.contains("<form") || text.contains("<body")
}

fn is_session_expired(status: StatusCode, location: Option<&HeaderValue>, body: &[u8]) -> bool {
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return true;
    }
    if location
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("login"))
    {
        return true;
    }
    let text = String::from_utf8_lossy(body).to_ascii_lowercase();
    text.contains("name=\"username\"")
        || text.contains("name='username'")
        || text.contains("name=\"password\"")
        || text.contains("act=login")
        || text.contains("<title>log in")
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn origin_of(url: &Url) -> Option<String> {
    let host = url.host_str()?;
    let mut origin = format!("{}://{}", url.scheme(), host);
    if let Some(port) = url.port() {
        origin.push(':');
        origin.push_str(&port.to_string());
    }
    Some(origin)
}

fn is_loopback_host(url: &Url) -> bool {
    matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"))
}

fn build_forum_http_client() -> Result<Client, reqwest::Error> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    Client::builder().no_proxy().redirect(Policy::none()).build()
}
