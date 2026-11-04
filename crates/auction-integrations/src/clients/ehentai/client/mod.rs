//! E-Hentai HTTP 客户端与论坛协议事务；固定 origin、限流、响应大小边界、
//! 主题创建/读取和 POST 后的 write-unknown 栅栏集中在此，表单与判定辅助在 forms.rs。

use auction_application::diagnostics::DiagnosticContext;

use crate::clients::metrics::{ForumTopicScanGuard, forum_operation, forum_world};
use crate::diagnostics as safe_diagnostics;

use std::{collections::VecDeque, sync::atomic::AtomicUsize};

use self::parent::*;
use super as parent;
use super::forms::{
    PostForm, TopicForm, body_looks_like_html, build_forum_http_client, clean_line, cookie_header,
    fixed_post_form_values, fixed_post_metadata, form_selector, hidden_input_selector,
    is_loopback_host, is_session_expired, link_selector, optional_query_identity, origin_of,
    post_body_selector, posting_action_matches_board, posting_action_matches_target,
    posting_form_values, same_origin, set_form_value, topic_title_selector, valid_cookie,
    valid_post_id, valid_rendered_post, valid_rendered_thread, valid_topic_description,
    valid_topic_id, valid_topic_title, valid_topic_titles,
};

mod pagination;
mod pm;
mod profile;
mod request;
mod session;
mod topic;

#[derive(Debug, Clone)]
pub struct EhentaiClientConfig {
    /// 固定论坛 origin；生产环境必须使用 HTTPS。
    pub endpoint: Url,
    /// 单次响应最大字节数，不能超过 8 MiB。
    pub maximum_response_bytes: usize,
    /// 单次主题读取允许抓取的最大页数，不能超过硬上限 20。
    pub maximum_topic_pages: usize,
    pub minimum_request_interval: Duration,
    pub request_timeout: Duration,
    /// 是否把所有世界映射到旧版测试板块 4。
    pub test_board_mode: bool,
    /// 主世界拍卖帖默认板块；生产默认 77。
    pub main_board_id: i32,
    /// 异世界拍卖帖默认板块；生产默认 90。
    pub isekai_board_id: i32,
}

impl EhentaiClientConfig {
    /// 返回生产固定 origin 的默认配置。
    pub fn production() -> Result<Self, EhentaiError> {
        let endpoint = Url::parse(PRODUCTION_ENDPOINT).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "forum_session",
                "parse_endpoint",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::configuration(EhentaiOperation::CreateTopic)
        })?;
        Ok(Self {
            endpoint,
            maximum_response_bytes: DEFAULT_MAXIMUM_RESPONSE_BYTES,
            maximum_topic_pages: DEFAULT_MAXIMUM_TOPIC_PAGES,
            minimum_request_interval: DEFAULT_MINIMUM_REQUEST_INTERVAL,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            test_board_mode: false,
            main_board_id: MAIN_BOARD_ID,
            isekai_board_id: ISEKAI_BOARD_ID,
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

/// 单次论坛请求的指标世界与账号 Cookie；只收敛参数，不改变所有权或请求顺序。
#[derive(Clone, Copy)]
pub(super) struct ForumRequestScope<'a> {
    pub(super) metrics_world: auction_application::ForumWorld,
    pub(super) cookie: &'a EhentaiCookie,
}

impl<'a> ForumRequestScope<'a> {
    pub(super) const fn new(
        metrics_world: auction_application::ForumWorld,
        cookie: &'a EhentaiCookie,
    ) -> Self {
        Self { metrics_world, cookie }
    }
}

#[derive(Clone)]
pub struct EhentaiClient {
    client: Client,
    // PM 不信任兼容构造器传入的 redirect 策略；克隆共享此安全连接池及原 gate/owner。
    pm_client: Option<Client>,
    base_url: Url,
    origin: String,
    gate: Arc<Mutex<Instant>>,
    maximum_response_bytes: usize,
    maximum_topic_pages: usize,
    request_timeout: Duration,
    minimum_request_interval: Duration,
    test_board_mode: bool,
    main_board_id: i32,
    isekai_board_id: i32,
    ownership: Option<Arc<dyn AdminCredentialRuntimeOwnership>>,
    write_progress: Option<auction_application::ForumWriteProgress>,
    external_metrics: Arc<dyn auction_application::ExternalMetricsObserver>,
}

impl EhentaiClient {
    /// 使用已有 HTTP client 建立兼容构造器。
    /// 生产装配应使用 [`Self::with_config`]，它会强制关闭自动重定向和传输层重试；
    /// 注入 client 的两项策略由调用方保证，该方法仅供已有装配和受控协议适配器使用。
    pub fn new(base_url: Url, client: Client) -> Self {
        let origin = origin_of(&base_url).unwrap_or_default();
        Self {
            client,
            pm_client: build_forum_http_client()
                .inspect_err(|cause| {
                    safe_diagnostics::warn(
                        "forum_pm",
                        "build_optional_client",
                        safe_diagnostics::project(cause),
                        DiagnosticContext::default(),
                    );
                })
                .ok(),
            base_url,
            origin,
            gate: Arc::new(Mutex::new(Instant::now())),
            maximum_response_bytes: DEFAULT_MAXIMUM_RESPONSE_BYTES,
            maximum_topic_pages: DEFAULT_MAXIMUM_TOPIC_PAGES,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            minimum_request_interval: DEFAULT_MINIMUM_REQUEST_INTERVAL,
            test_board_mode: false,
            main_board_id: MAIN_BOARD_ID,
            isekai_board_id: ISEKAI_BOARD_ID,
            ownership: None,
            write_progress: None,
            external_metrics: auction_application::noop_external_metrics_observer(),
        }
    }

    /// 使用安全默认 HTTP client 建立客户端。
    pub fn with_config(config: EhentaiClientConfig) -> Result<Self, EhentaiError> {
        config.validate(EhentaiOperation::CreateTopic)?;
        let client = build_forum_http_client().map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "forum_session",
                "build_client",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::configuration(EhentaiOperation::CreateTopic)
        })?;
        Self::from_config_and_client(config, client)
    }

    /// 使用调用方提供的 client 建立客户端；调用方必须禁止重定向和传输层重试。
    pub fn from_config_and_client(
        config: EhentaiClientConfig,
        client: Client,
    ) -> Result<Self, EhentaiError> {
        config.validate(EhentaiOperation::CreateTopic)?;
        let origin = origin_of(&config.endpoint)
            .ok_or(EhentaiError::configuration(EhentaiOperation::CreateTopic))?;
        let pm_client = build_forum_http_client().map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "forum_session",
                "build_pm_client",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::configuration(EhentaiOperation::CheckSession)
        })?;
        Ok(Self {
            client,
            pm_client: Some(pm_client),
            base_url: config.endpoint,
            origin,
            gate: Arc::new(Mutex::new(Instant::now())),
            maximum_response_bytes: config.maximum_response_bytes,
            maximum_topic_pages: config.maximum_topic_pages,
            request_timeout: config.request_timeout,
            minimum_request_interval: config.minimum_request_interval,
            test_board_mode: config.test_board_mode,
            main_board_id: config.main_board_id,
            isekai_board_id: config.isekai_board_id,
            ownership: None,
            write_progress: None,
            external_metrics: auction_application::noop_external_metrics_observer(),
        })
    }

    /// 网关注入凭据 owner；分页上下文与克隆客户端共享原持锁 session 的有效性。
    pub(crate) fn with_ownership(
        mut self,
        ownership: Option<Arc<dyn AdminCredentialRuntimeOwnership>>,
    ) -> Self {
        if ownership.is_some() {
            self.ownership = ownership;
        }
        self
    }

    /// 为一次论坛写入注入独立进度；克隆仍共享连接池、限速器和凭据所有权。
    pub(crate) fn with_write_progress(
        mut self,
        progress: auction_application::ForumWriteProgress,
    ) -> Self {
        self.write_progress = Some(progress);
        self
    }

    /// 注入同步外部客户端指标；只读取配置间隔，不改变请求门。
    pub fn with_external_metrics(
        mut self,
        observer: Arc<dyn auction_application::ExternalMetricsObserver>,
    ) -> Self {
        observer.observe(auction_application::ExternalMetricsEvent::ForumRequestInterval {
            interval: self.minimum_request_interval,
        });
        self.external_metrics = observer;
        self
    }

    pub(crate) fn external_metrics(&self) -> Arc<dyn auction_application::ExternalMetricsObserver> {
        Arc::clone(&self.external_metrics)
    }

    async fn check_ownership(&self, cookie: &EhentaiCookie) -> Result<(), ()> {
        if !cookie.is_active() {
            return Err(());
        }
        if let Some(ownership) = &self.ownership {
            // 预算同时覆盖持锁连接的排队与探测；超时取消会永久撤销 owner。
            tokio::time::timeout(Duration::from_secs(1), ownership.check())
                .await
                .map_err(|diagnostic_cause| {
                    safe_diagnostics::warn(
                        "forum_session",
                        "check_owner_timeout",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                })?
                .map_err(|diagnostic_cause| {
                    safe_diagnostics::warn(
                        "forum_session",
                        "check_owner",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                })?;
        }
        cookie.is_active().then_some(()).ok_or(())
    }

    /// 返回底层 HTTP client，供需要共享连接池的装配代码使用。
    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    /// 创建或按精确标题查找一个论坛主题。
    pub async fn create_topic(
        &self,
        world: SettlementWorld,
        cookie: &EhentaiCookie,
        rendered: ForumRenderedThread,
    ) -> Result<auction_application::ForumTopicPublication, EhentaiError> {
        let operation = EhentaiOperation::CreateTopic;
        if !self.valid_for() || !valid_cookie(cookie) || !valid_rendered_thread(&rendered) {
            return Err(EhentaiError::configuration(operation));
        }
        let (board_id, board_url, existing) = self
            .find_topic_by_titles_with_operation(
                world,
                cookie,
                &rendered.title,
                &rendered.reconciliation_titles,
                operation,
            )
            .await?;
        if let Some(published) = existing {
            return Ok(auction_application::ForumTopicPublication::Existing(published));
        }

        let form = self.load_topic_form(board_id, &board_url, forum_world(world), cookie).await?;
        self.submit_topic(form, board_id, rendered, forum_world(world), cookie)
            .await
            .map(auction_application::ForumTopicPublication::Created)
    }

    /// 一次版面读取核对全部精确标题及唯一主题身份；永远不加载发帖表单或发送 POST。
    pub async fn find_topic_by_titles(
        &self,
        world: SettlementWorld,
        cookie: &EhentaiCookie,
        titles: &[String],
    ) -> Result<Option<auction_application::ForumPublishedThread>, EhentaiError> {
        let (title, reconciliation_titles) = titles
            .split_first()
            .ok_or_else(|| EhentaiError::configuration(EhentaiOperation::ReconcileTopic))?;
        let (_, _, published) = self
            .find_topic_by_titles_with_operation(
                world,
                cookie,
                title,
                reconciliation_titles,
                EhentaiOperation::ReconcileTopic,
            )
            .await?;
        Ok(published)
    }

    async fn find_topic_by_titles_with_operation(
        &self,
        world: SettlementWorld,
        cookie: &EhentaiCookie,
        title: &str,
        reconciliation_titles: &[String],
        operation: EhentaiOperation,
    ) -> Result<(i32, Url, Option<auction_application::ForumPublishedThread>), EhentaiError> {
        if !self.valid_for()
            || !valid_cookie(cookie)
            || !valid_topic_titles(title, reconciliation_titles)
        {
            return Err(EhentaiError::configuration(operation));
        }
        let board_id = self.board_for_world(world);
        let board_value = board_id.to_string();
        let board_url = self.index_url(&[("showforum", board_value.as_str())], operation)?;
        let (body, _) = self
            .get_page(
                operation,
                forum_operation(operation),
                ForumRequestScope::new(forum_world(world), cookie),
                board_url.clone(),
                None,
            )
            .await?;
        let published =
            self.find_topic(&board_url, board_id, title, reconciliation_titles, &body, operation)?;
        Ok((board_id, board_url, published))
    }

    /// 读取一个已发布主题，并复用论坛预览适配器解析帖子命令。
    pub async fn fetch_topic(
        &self,
        world: SettlementWorld,
        thread_id: &str,
        cookie: &EhentaiCookie,
    ) -> Result<AdminForumPreviewResult, EhentaiError> {
        let mut scan = ForumTopicScanGuard::start(self.external_metrics(), forum_world(world));
        let result = self.fetch_topic_inner(world, thread_id, cookie, &mut scan).await;
        if result.is_ok() {
            scan.finish_complete();
        } else {
            scan.finish_error();
        }
        result
    }

    /// 按游标读取主题：固定楼层页 + 游标页及其之后的分页。
    ///
    /// 先核对游标页仍包含游标帖子；删帖导致楼层前移或帖子缺失时回退完整读取，
    /// 因此增量只是减少分页请求，不改变任何帖子、命令或固定楼层的解释结果。
    pub async fn fetch_topic_incremental(
        &self,
        world: SettlementWorld,
        thread_id: &str,
        cookie: &EhentaiCookie,
        cursor: Option<&ForumTopicCursor>,
        fixed_through_post_number: u32,
    ) -> Result<ForumTopicScan, EhentaiError> {
        let mut scan = ForumTopicScanGuard::start(self.external_metrics(), forum_world(world));
        let result = self
            .fetch_topic_scan(
                world,
                thread_id,
                cookie,
                &mut scan,
                cursor,
                fixed_through_post_number,
            )
            .await;
        if result.is_ok() {
            scan.finish_complete();
        } else {
            scan.finish_error();
        }
        result
    }

    async fn fetch_topic_inner(
        &self,
        world: SettlementWorld,
        thread_id: &str,
        cookie: &EhentaiCookie,
        scan: &mut ForumTopicScanGuard,
    ) -> Result<AdminForumPreviewResult, EhentaiError> {
        self.fetch_topic_scan(world, thread_id, cookie, scan, None, 0)
            .await
            .map(|scan| scan.preview)
    }

    async fn fetch_topic_scan(
        &self,
        world: SettlementWorld,
        thread_id: &str,
        cookie: &EhentaiCookie,
        scan: &mut ForumTopicScanGuard,
        cursor: Option<&ForumTopicCursor>,
        fixed_through_post_number: u32,
    ) -> Result<ForumTopicScan, EhentaiError> {
        let operation = EhentaiOperation::FetchTopic;
        if !self.valid_for() || !valid_cookie(cookie) || !valid_topic_id(thread_id) {
            return Err(EhentaiError::configuration(operation));
        }
        let board_id = self.board_for_world(world);
        if board_id <= 0 {
            return Err(EhentaiError::configuration(operation));
        }
        let target = self.index_url(&[("showtopic", thread_id)], operation)?;
        let (body, _) = self
            .get_page(
                operation,
                forum_operation(operation),
                ForumRequestScope::new(forum_world(world), cookie),
                target.clone(),
                Some(&target),
            )
            .await?;
        let html = String::from_utf8(body).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "forum_topic",
                "decode_first_page",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::protocol(operation, false)
        })?;
        let parser = crate::ForumPreviewAdapter::new();
        let first_page = parser.parse_html_page(&html).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "forum_topic",
                "parse_first_page",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::protocol(operation, false)
        })?;
        scan.set_pages(1);
        let targets =
            self.topic_page_targets(&first_page, thread_id, operation, forum_world(world))?;
        let maximum_offset = targets.last().map(|(offset, _)| *offset).unwrap_or(0);
        let mut pages = Vec::with_capacity(targets.len());
        let mut total_bytes = html.len();
        drop(html);
        pages.push((0_u64, first_page));
        scan.set_pages(pages.len());
        let read_context = TopicPageReadContext {
            client: self.clone(),
            cookie: cookie.clone(),
            operation,
            metrics_world: forum_world(world),
            thread_id: thread_id.to_owned(),
            known_maximum_offset: maximum_offset,
            referer: target,
            // 在逐块读取时共享剩余额度，不能等并发整页下载、解析后才发现超限。
            remaining_bytes: Arc::new(AtomicUsize::new(HARD_MAXIMUM_TOPIC_BYTES - total_bytes)),
        };
        // 增量预算：只读固定楼层页与游标页之后；游标页先单独核对，失败即整场重读。
        let (selected_pages, probed_offset) = self
            .incremental_scan_plan(
                &targets,
                cursor,
                fixed_through_post_number,
                &read_context,
                pagination::IncrementalScanState {
                    pages: &mut pages,
                    total_bytes: &mut total_bytes,
                },
                operation,
            )
            .await?;
        let mode = if selected_pages.len() == targets.len() {
            ForumTopicScanMode::Full
        } else {
            scan.set_mode(ForumTopicScanMode::Incremental);
            ForumTopicScanMode::Incremental
        };
        let expected_pages = selected_pages.len();
        let mut remaining = targets
            .into_iter()
            .enumerate()
            .filter(|(index, _)| *index != 0)
            .filter(|(index, _)| selected_pages.binary_search(&(*index as u32)).is_ok())
            .filter(|(_, (offset, _))| Some(*offset) != probed_offset)
            .map(|(_, target)| target)
            .collect::<VecDeque<_>>();
        let mut reads = tokio::task::JoinSet::new();
        spawn_topic_page_reads(&mut reads, &mut remaining, &read_context);
        while let Some(read) = reads.join_next().await {
            let page = read.map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "forum_topic",
                    "read_page_join",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                EhentaiError::remote_temporary(operation, false)
            })??;
            total_bytes = total_bytes
                .checked_add(page.1)
                .filter(|total| *total <= HARD_MAXIMUM_TOPIC_BYTES)
                .ok_or_else(|| EhentaiError::response_too_large(operation))?;
            pages.push((page.0, page.2));
            scan.set_pages(pages.len());
            spawn_topic_page_reads(&mut reads, &mut remaining, &read_context);
        }
        pages.sort_unstable_by_key(|(offset, _)| *offset);
        if pages.len() != expected_pages || pages.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
            return Err(EhentaiError::protocol(operation, false));
        }
        // 绝对楼层由分页偏移与本页序号换算；目录首帖（楼层 1）不进入 preview.posts。
        let page_post_counts =
            pages.iter().map(|(offset, page)| (*offset, page.post_count())).collect::<Vec<_>>();
        let post_numbers = topic_post_numbers(&page_post_counts, operation)?;
        let parsed_pages = pages.into_iter().map(|(_, page)| page).collect();
        let preview = parser.parse_html_pages(parsed_pages).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "forum_topic",
                "merge_pages",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::protocol(operation, false)
        })?;
        let result = ForumTopicScan { preview, post_numbers, mode };
        result.validate().map_err(|error| {
            let _ = error;
            EhentaiError::protocol(operation, false)
        })?;
        Ok(result)
    }

    /// 建立目录续页或系统日志回复；POST 后必须返回 exact thread/post proof。
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
        let board_id = self.board_for_world(world);
        let form = self.load_reply_form(board_id, thread_id, forum_world(world), cookie).await?;
        self.submit_fixed_post(
            form,
            board_id,
            thread_id,
            None,
            body,
            ForumRequestScope::new(forum_world(world), cookie),
        )
        .await
    }

    /// 编辑已持久化的固定目录页或系统日志；目标 post ID 必须由调用方提供，不能从正文猜测。
    pub async fn edit_post(
        &self,
        world: SettlementWorld,
        thread_id: &str,
        post_id: &str,
        metadata: Option<&ForumTopicMetadata>,
        body: ForumRenderedPost,
        cookie: &EhentaiCookie,
    ) -> Result<ForumPublishedPost, EhentaiError> {
        let operation = EhentaiOperation::EditPost;
        if !self.valid_for()
            || !valid_cookie(cookie)
            || !valid_topic_id(thread_id)
            || !valid_post_id(post_id)
            || metadata.is_some_and(|metadata| {
                metadata.title.as_deref().is_some_and(|value| !valid_topic_title(value))
                    || metadata
                        .description
                        .as_deref()
                        .is_some_and(|value| !valid_topic_description(value))
            })
            || !valid_rendered_post(&body)
        {
            return Err(EhentaiError::configuration(operation));
        }
        let board_id = self.board_for_world(world);
        let form =
            self.load_edit_form(board_id, thread_id, post_id, forum_world(world), cookie).await?;
        if metadata
            .is_some_and(|metadata| metadata.title.is_some() || metadata.description.is_some())
            && form.metadata.is_none()
        {
            // 缺少首帖元数据字段时不能假定服务器会接受额外标题字段。
            return Err(EhentaiError::protocol(operation, false));
        }
        self.submit_fixed_post(
            form,
            board_id,
            thread_id,
            Some((
                post_id,
                metadata.and_then(|metadata| metadata.title.as_deref()),
                metadata.and_then(|metadata| metadata.description.as_deref()),
            )),
            body,
            ForumRequestScope::new(forum_world(world), cookie),
        )
        .await
    }

    /// 读取已校验编辑表单的原始 BBCode 与首帖元数据，不能以正文代替标题证明。
    pub async fn read_post(
        &self,
        world: SettlementWorld,
        thread_id: &str,
        post_id: &str,
        cookie: &EhentaiCookie,
    ) -> Result<ForumReadPost, EhentaiError> {
        let operation = EhentaiOperation::ReadPost;
        if !self.valid_for()
            || !valid_cookie(cookie)
            || !valid_topic_id(thread_id)
            || !valid_post_id(post_id)
        {
            return Err(EhentaiError::configuration(operation));
        }
        let board_id = self.board_for_world(world);
        let form =
            self.load_edit_form(board_id, thread_id, post_id, forum_world(world), cookie).await?;
        let body = ForumRenderedPost { body: form.body };
        if !valid_rendered_post(&body) {
            return Err(EhentaiError::protocol(operation, false));
        }
        Ok(ForumReadPost { body: body.body, metadata: form.metadata })
    }
}

#[derive(Clone)]
struct TopicPageReadContext {
    client: EhentaiClient,
    cookie: EhentaiCookie,
    operation: EhentaiOperation,
    metrics_world: auction_application::ForumWorld,
    thread_id: String,
    known_maximum_offset: u64,
    referer: Url,
    remaining_bytes: Arc<AtomicUsize>,
}

async fn read_topic_page(
    context: TopicPageReadContext,
    offset: u64,
    page_target: Url,
) -> Result<(u64, usize, crate::forum::ParsedHtmlPage), EhentaiError> {
    let identity_target = page_target.clone();
    let (body, _) = context
        .client
        .get_page_with_budget(
            context.operation,
            forum_operation(context.operation),
            ForumRequestScope::new(context.metrics_world, &context.cookie),
            page_target,
            Some(&context.referer),
            Some(&context.remaining_bytes),
        )
        .await?;
    let html = String::from_utf8(body).map_err(|diagnostic_cause| {
        safe_diagnostics::warn(
            "forum_topic",
            "decode_extra_page",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        EhentaiError::protocol(context.operation, false)
    })?;
    let page_bytes = html.len();
    let page =
        crate::ForumPreviewAdapter::new().parse_html_page(&html).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "forum_topic",
                "parse_extra_page",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::protocol(context.operation, false)
        })?;
    context.client.validate_topic_page_identity(&page, &identity_target, offset, &context)?;
    Ok((offset, page_bytes, page))
}

/// 把分页覆盖换算成绝对楼层（1 基）。目录首帖占楼层 1，但不进入 `preview.posts`，
/// 因此结果从楼层 2 开始，与 fixture 中的行保持一一对应。
fn topic_post_numbers(
    pages: &[(u64, usize)],
    operation: EhentaiOperation,
) -> Result<Vec<u32>, EhentaiError> {
    let mut post_numbers = Vec::new();
    for (offset, count) in pages {
        for index in 0..*count {
            let floor = offset
                .checked_add(u64::try_from(index).map_err(|error| {
                    let _ = error;
                    EhentaiError::protocol(operation, false)
                })?)
                .and_then(|value| value.checked_add(1))
                .ok_or_else(|| EhentaiError::protocol(operation, false))?;
            if floor == 1 {
                continue;
            }
            post_numbers.push(u32::try_from(floor).map_err(|error| {
                let _ = error;
                EhentaiError::protocol(operation, false)
            })?);
        }
    }
    Ok(post_numbers)
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
            .field("main_board_id", &self.main_board_id)
            .field("isekai_board_id", &self.isekai_board_id)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use super as parent;

    #[test]
    fn full_scan_floors_follow_page_offsets() {
        // 三页各 20 楼：楼层 1 是目录首帖，不进入 preview.posts。
        let numbers =
            topic_post_numbers(&[(0, 20), (20, 20), (40, 20)], EhentaiOperation::FetchTopic)
                .expect("楼层换算在预算内");
        assert_eq!(numbers.len(), 59);
        assert_eq!(numbers.first().copied(), Some(2));
        assert_eq!(numbers.last().copied(), Some(60));
    }

    #[test]
    fn incremental_scan_keeps_absolute_floors_after_skipped_pages() {
        // 固定首帖页 + 游标页（第 6 页，0 基 5）：中间分页被跳过，楼层仍为绝对楼层。
        let numbers =
            topic_post_numbers(&[(0, 20), (100, 20), (120, 7)], EhentaiOperation::FetchTopic)
                .expect("楼层换算在预算内");
        assert_eq!(numbers.len(), 19 + 20 + 7);
        assert_eq!(numbers[18], 20);
        assert_eq!(numbers[19], 101);
        assert_eq!(numbers.last().copied(), Some(127));
    }
}
