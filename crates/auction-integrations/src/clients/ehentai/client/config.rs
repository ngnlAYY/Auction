//! 论坛客户端配置：固定 origin、单响应与主题分页上限，以及请求时间边界。

use auction_application::core::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use std::time::Duration;

use url::Url;

use super::super::forms::is_loopback_host;
use super::super::{
    DEFAULT_MAXIMUM_RESPONSE_BYTES, DEFAULT_MAXIMUM_TOPIC_PAGES, DEFAULT_MINIMUM_REQUEST_INTERVAL,
    DEFAULT_REQUEST_TIMEOUT, EhentaiError, EhentaiOperation, HARD_MAXIMUM_RESPONSE_BYTES,
    HARD_MAXIMUM_TOPIC_PAGES, ISEKAI_BOARD_ID, MAIN_BOARD_ID, PRODUCTION_ENDPOINT,
};

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
    pub(super) fn validate(&self, operation: EhentaiOperation) -> Result<(), EhentaiError> {
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
