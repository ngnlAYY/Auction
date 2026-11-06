//! E-Hentai HTTP 客户端与论坛协议事务；固定 origin、限流、响应大小边界、
//! 主题创建/读取和 POST 后的 write-unknown 栅栏集中在此，表单与判定辅助在 forms.rs。
//!
//! 限速与 MoogleMail 共用 `crate::clients::gate` 的分级请求门：建主题、回帖、编辑与
//! PM 发送走前台档，页面预读与增量扫描走后台档。论坛是独立 origin，没有 HentaiVerse
//! 战斗页信号，因此不接账号忙熔断；429 与 Cloudflare 等分类和写入未知语义保持原样。
//!
//! 客户端类型与请求作用域留在本模块；构造、配置、主题操作及整页/增量读取分别位于
//! `construction`、`config`、`operations`、`scan`。

use std::sync::atomic::AtomicUsize;

use self::parent::*;
use super as parent;
use super::forms::{
    PostForm, TopicForm, body_looks_like_html, clean_line, cookie_header, fixed_post_form_values,
    fixed_post_metadata, form_selector, hidden_input_selector, is_loopback_host,
    is_session_expired, link_selector, optional_query_identity, post_body_selector,
    posting_action_matches_board, posting_action_matches_target, posting_form_values, same_origin,
    set_form_value, topic_title_selector, valid_cookie, valid_post_id, valid_topic_id,
};

mod config;
mod construction;
mod operations;
mod pagination;
mod pm;
mod profile;
mod request;
mod scan;
mod session;
mod topic;

pub use self::config::EhentaiClientConfig;

#[derive(Clone)]
pub struct EhentaiClient {
    client: Client,
    // PM 不信任兼容构造器传入的 redirect 策略；克隆共享此安全连接池及原 gate/owner。
    pm_client: Option<Client>,
    base_url: Url,
    origin: String,
    /// 与 MoogleMail 相同的容量 1 分级请求门；克隆共享同一份限速状态。
    gate: Arc<crate::clients::gate::PriorityGate>,
    maximum_response_bytes: usize,
    maximum_topic_pages: usize,
    request_timeout: Duration,
    minimum_request_interval: Duration,
    test_board_mode: bool,
    main_board_id: i32,
    isekai_board_id: i32,
    ownership: Option<Arc<dyn AdminCredentialRuntimeOwnership>>,
    write_progress: Option<auction_application::forum::ForumWriteProgress>,
    external_metrics: Arc<dyn auction_application::external_metrics::ExternalMetricsObserver>,
}

/// 单次论坛请求的指标世界与账号 Cookie；只收敛参数，不改变所有权或请求顺序。
#[derive(Clone, Copy)]
pub(super) struct ForumRequestScope<'a> {
    pub(super) metrics_world: auction_application::external_metrics::ForumWorld,
    pub(super) cookie: &'a EhentaiCookie,
}

impl<'a> ForumRequestScope<'a> {
    pub(super) const fn new(
        metrics_world: auction_application::external_metrics::ForumWorld,
        cookie: &'a EhentaiCookie,
    ) -> Self {
        Self { metrics_world, cookie }
    }
}

/// 主题分页并发读取共享的请求身份、参照页与剩余字节额度。
#[derive(Clone)]
struct TopicPageReadContext {
    client: EhentaiClient,
    cookie: EhentaiCookie,
    operation: EhentaiOperation,
    metrics_world: auction_application::external_metrics::ForumWorld,
    thread_id: String,
    known_maximum_offset: u64,
    referer: Url,
    remaining_bytes: Arc<AtomicUsize>,
}
