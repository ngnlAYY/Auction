//! E-Hentai HTTP 客户端与论坛协议事务。
//!
//! 本模块只负责固定 origin、请求限流、响应大小边界、主题创建/读取以及 POST 后的
//! write-unknown 栅栏；表单字段和响应判定辅助函数集中在 forms.rs。

use super::forms::{
    TopicForm, body_looks_like_html, build_forum_http_client, clean_line, cookie_header,
    is_loopback_host, is_session_expired, origin_of, posting_action_matches_board,
    posting_form_values, same_origin, set_form_value, valid_cookie, valid_rendered_thread,
    valid_topic_id,
};
use super::*;

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
