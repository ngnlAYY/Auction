//! E-Hentai HTTP 客户端与论坛协议事务。
//!
//! 本模块只负责固定 origin、请求限流、响应大小边界、主题创建/读取以及 POST 后的
//! write-unknown 栅栏；表单字段和响应判定辅助函数集中在 forms.rs。

use std::collections::VecDeque;

use super::forms::{
    PostForm, TopicForm, body_looks_like_html, build_forum_http_client, clean_line, cookie_header,
    fixed_post_form_values, form_selector, hidden_input_selector, is_loopback_host,
    is_session_expired, link_selector, optional_query_identity, origin_of, post_body_selector,
    posting_action_matches_board, posting_action_matches_target, posting_form_values, same_origin,
    set_form_value, topic_title_selector, valid_cookie, valid_post_id, valid_rendered_post,
    valid_rendered_thread, valid_topic_id, valid_topic_title,
};
use super::*;

mod pagination;
mod request;
mod topic;

/// E-Hentai HTTP 客户端配置。
#[derive(Debug, Clone)]
pub struct EhentaiClientConfig {
    /// 固定论坛 origin；生产环境必须使用 HTTPS。
    pub endpoint: Url,
    /// 单次响应最大字节数，不能超过 8 MiB。
    pub maximum_response_bytes: usize,
    /// 单次主题读取允许抓取的最大页数，不能超过硬上限 20。
    pub maximum_topic_pages: usize,
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
            maximum_topic_pages: DEFAULT_MAXIMUM_TOPIC_PAGES,
            minimum_request_interval: DEFAULT_MINIMUM_REQUEST_INTERVAL,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            test_board_mode: false,
        })
    }

    /// 校验固定 origin、资源上限和时间边界。
    fn validate(&self, operation: EhentaiOperation) -> Result<(), EhentaiError> {
        let valid_scheme = self.endpoint.scheme() == "https"
            || (self.endpoint.scheme() == "http" && is_loopback_host(&self.endpoint));
        let path_is_root = self.endpoint.path().is_empty() || self.endpoint.path() == "/";
        if !valid_scheme
            || self.endpoint.host_str().is_none()
            || !self.endpoint.username().is_empty()
            || self.endpoint.password().is_some()
            || !path_is_root
            || self.endpoint.query().is_some()
            || self.endpoint.fragment().is_some()
            || self.maximum_response_bytes == 0
            || self.maximum_response_bytes > HARD_MAXIMUM_RESPONSE_BYTES
            || self.maximum_topic_pages == 0
            || self.maximum_topic_pages > HARD_MAXIMUM_TOPIC_PAGES
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
    maximum_topic_pages: usize,
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
            maximum_topic_pages: DEFAULT_MAXIMUM_TOPIC_PAGES,
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
            maximum_topic_pages: config.maximum_topic_pages,
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
        let (board_id, board_url, existing) = self
            .find_topic_by_title_with_operation(world, cookie, &rendered.title, operation)
            .await?;
        if let Some(published) = existing {
            return Ok(published);
        }

        let form = self.load_topic_form(board_id, &board_url, cookie).await?;
        self.submit_topic(form, board_id, rendered, cookie).await
    }

    /// 只读按精确标题查找主题；该入口永远不会加载发帖表单或发送 POST。
    pub async fn find_topic_by_title(
        &self,
        world: SettlementWorld,
        cookie: &EhentaiCookie,
        title: &str,
    ) -> Result<Option<auction_application::ForumPublishedThread>, EhentaiError> {
        let (_, _, published) = self
            .find_topic_by_title_with_operation(
                world,
                cookie,
                title,
                EhentaiOperation::ReconcileTopic,
            )
            .await?;
        Ok(published)
    }

    async fn find_topic_by_title_with_operation(
        &self,
        world: SettlementWorld,
        cookie: &EhentaiCookie,
        title: &str,
        operation: EhentaiOperation,
    ) -> Result<(i32, Url, Option<auction_application::ForumPublishedThread>), EhentaiError> {
        if !self.valid_for() || !valid_cookie(cookie) || !valid_topic_title(title) {
            return Err(EhentaiError::configuration(operation));
        }
        let board_id = self.board_for_world(world, operation)?;
        let board_value = board_id.to_string();
        let board_url = self.index_url(&[("showforum", board_value.as_str())], operation)?;
        let (body, _) = self.get_page(operation, board_url.clone(), None, cookie).await?;
        let published = self.find_topic(&board_url, board_id, title, &body, operation)?;
        Ok((board_id, board_url, published))
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
        if board_id <= 0 {
            return Err(EhentaiError::configuration(operation));
        }
        let target = self.index_url(&[("showtopic", thread_id)], operation)?;
        let (body, _) = self.get_page(operation, target.clone(), Some(&target), cookie).await?;
        let html = String::from_utf8(body).map_err(|_| EhentaiError::protocol(operation, false))?;
        let parser = crate::ForumPreviewAdapter::new();
        let first_page =
            parser.parse_html_page(&html).map_err(|_| EhentaiError::protocol(operation, false))?;
        let targets = self.topic_page_targets(&first_page, thread_id, operation)?;
        let expected_pages = targets.len();
        let maximum_offset = targets.last().map(|(offset, _)| *offset).unwrap_or(0);
        let mut remaining = targets.into_iter().skip(1).collect::<VecDeque<_>>();
        let mut pages = Vec::with_capacity(expected_pages);
        let mut total_bytes = html.len();
        pages.push((0_u64, first_page));
        let mut reads = tokio::task::JoinSet::new();
        let read_context = TopicPageReadContext {
            client: self.clone(),
            cookie: cookie.clone(),
            operation,
            thread_id: thread_id.to_owned(),
            known_maximum_offset: maximum_offset,
            referer: target,
        };
        spawn_topic_page_reads(&mut reads, &mut remaining, &read_context);
        while let Some(read) = reads.join_next().await {
            let page = read.map_err(|_| EhentaiError::remote_temporary(operation, false))??;
            total_bytes = total_bytes
                .checked_add(page.1)
                .filter(|total| *total <= HARD_MAXIMUM_TOPIC_BYTES)
                .ok_or_else(|| EhentaiError::response_too_large(operation))?;
            pages.push((page.0, page.2));
            spawn_topic_page_reads(&mut reads, &mut remaining, &read_context);
        }
        pages.sort_unstable_by_key(|(offset, _)| *offset);
        if pages.len() != expected_pages || pages.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
            return Err(EhentaiError::protocol(operation, false));
        }
        let parsed_pages = pages.into_iter().map(|(_, page)| page).collect();
        parser.parse_html_pages(parsed_pages).map_err(|_| EhentaiError::protocol(operation, false))
    }

    /// 建立固定 #2 回复；POST 后必须返回 exact thread/post proof。
    pub async fn reply_topic(
        &self,
        world: SettlementWorld,
        thread_id: &str,
        body: ForumRenderedPost,
        cookie: &EhentaiCookie,
    ) -> Result<ForumPublishedPost, EhentaiError> {
        let operation = EhentaiOperation::ReplyTopic;
        if !self.valid_for()
            || !valid_cookie(cookie)
            || !valid_topic_id(thread_id)
            || !valid_rendered_post(&body)
        {
            return Err(EhentaiError::configuration(operation));
        }
        let board_id = self.board_for_world(world, operation)?;
        let form = self.load_reply_form(board_id, thread_id, cookie).await?;
        self.submit_fixed_post(form, board_id, thread_id, None, body, cookie).await
    }

    /// 编辑已持久化的固定 #1/#2；目标 post ID 必须由调用方提供，不能从正文猜测。
    pub async fn edit_post(
        &self,
        world: SettlementWorld,
        thread_id: &str,
        post_id: &str,
        title: Option<&str>,
        body: ForumRenderedPost,
        cookie: &EhentaiCookie,
    ) -> Result<ForumPublishedPost, EhentaiError> {
        let operation = EhentaiOperation::EditPost;
        if !self.valid_for()
            || !valid_cookie(cookie)
            || !valid_topic_id(thread_id)
            || !valid_post_id(post_id)
            || title.is_some_and(|value| !valid_topic_title(value))
            || !valid_rendered_post(&body)
        {
            return Err(EhentaiError::configuration(operation));
        }
        let board_id = self.board_for_world(world, operation)?;
        let form = self.load_edit_form(board_id, thread_id, post_id, cookie).await?;
        self.submit_fixed_post(form, board_id, thread_id, Some((post_id, title)), body, cookie)
            .await
    }

    /// 读取编辑表单中由服务器返回的原始 BBCode，作为固定楼层写入后的精确正文证明。
    pub async fn read_post_body(
        &self,
        world: SettlementWorld,
        thread_id: &str,
        post_id: &str,
        cookie: &EhentaiCookie,
    ) -> Result<ForumRenderedPost, EhentaiError> {
        let operation = EhentaiOperation::ReadPostBody;
        if !self.valid_for()
            || !valid_cookie(cookie)
            || !valid_topic_id(thread_id)
            || !valid_post_id(post_id)
        {
            return Err(EhentaiError::configuration(operation));
        }
        let board_id = self.board_for_world(world, operation)?;
        let form = self.load_edit_form(board_id, thread_id, post_id, cookie).await?;
        let body = ForumRenderedPost { body: form.body };
        if !valid_rendered_post(&body) {
            return Err(EhentaiError::protocol(operation, false));
        }
        Ok(body)
    }
}

#[derive(Clone)]
struct TopicPageReadContext {
    client: EhentaiClient,
    cookie: EhentaiCookie,
    operation: EhentaiOperation,
    thread_id: String,
    known_maximum_offset: u64,
    referer: Url,
}

async fn read_topic_page(
    context: TopicPageReadContext,
    offset: u64,
    page_target: Url,
) -> Result<(u64, usize, crate::forum::ParsedHtmlPage), EhentaiError> {
    let identity_target = page_target.clone();
    let (body, _) = context
        .client
        .get_page(context.operation, page_target, Some(&context.referer), &context.cookie)
        .await?;
    let html =
        String::from_utf8(body).map_err(|_| EhentaiError::protocol(context.operation, false))?;
    let page_bytes = html.len();
    let page = crate::ForumPreviewAdapter::new()
        .parse_html_page(&html)
        .map_err(|_| EhentaiError::protocol(context.operation, false))?;
    context.client.validate_topic_page_identity(
        &page,
        &identity_target,
        &context.thread_id,
        offset,
        context.known_maximum_offset,
        context.operation,
    )?;
    Ok((offset, page_bytes, page))
}

fn spawn_topic_page_reads(
    reads: &mut tokio::task::JoinSet<
        Result<(u64, usize, crate::forum::ParsedHtmlPage), EhentaiError>,
    >,
    remaining: &mut VecDeque<(u64, Url)>,
    context: &TopicPageReadContext,
) {
    while reads.len() < MAXIMUM_CONCURRENT_TOPIC_READS {
        let Some((offset, page_target)) = remaining.pop_front() else {
            break;
        };
        reads.spawn(read_topic_page(context.clone(), offset, page_target));
    }
}

impl fmt::Debug for EhentaiClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EhentaiClient")
            .field("base_url", &self.base_url)
            .field("maximum_response_bytes", &self.maximum_response_bytes)
            .field("maximum_topic_pages", &self.maximum_topic_pages)
            .field("request_timeout", &self.request_timeout)
            .field("minimum_request_interval", &self.minimum_request_interval)
            .field("test_board_mode", &self.test_board_mode)
            .finish()
    }
}
