//! 客户端构造、凭据 owner 绑定、请求门与外部指标注入；不包含具体论坛协议。

use auction_application::admin::AdminCredentialRuntimeOwnership;
use auction_application::core::diagnostics::DiagnosticContext;

use crate::safe_diagnostics;

use std::{fmt, sync::Arc, time::Duration};

use reqwest::Client;
use url::Url;

use super::super::forms::{build_forum_http_client, origin_of};
use super::super::{EhentaiError, EhentaiOperation};
use super::{EhentaiClient, EhentaiClientConfig, EhentaiCookie};

impl EhentaiClient {
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
            gate: Arc::new(crate::clients::gate::PriorityGate::new(
                config.minimum_request_interval,
            )),
            maximum_response_bytes: config.maximum_response_bytes,
            maximum_topic_pages: config.maximum_topic_pages,
            request_timeout: config.request_timeout,
            minimum_request_interval: config.minimum_request_interval,
            test_board_mode: config.test_board_mode,
            main_board_id: config.main_board_id,
            isekai_board_id: config.isekai_board_id,
            ownership: None,
            write_progress: None,
            external_metrics: auction_application::external_metrics::noop_external_metrics_observer(
            ),
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
        progress: auction_application::forum::ForumWriteProgress,
    ) -> Self {
        self.write_progress = Some(progress);
        self
    }

    /// 注入同步外部客户端指标；只读取配置间隔，不改变请求门。
    pub fn with_external_metrics(
        mut self,
        observer: Arc<dyn auction_application::external_metrics::ExternalMetricsObserver>,
    ) -> Self {
        observer.observe(
            auction_application::external_metrics::ExternalMetricsEvent::ForumRequestInterval {
                interval: self.minimum_request_interval,
            },
        );
        self.external_metrics = observer;
        self
    }

    pub(crate) fn external_metrics(
        &self,
    ) -> Arc<dyn auction_application::external_metrics::ExternalMetricsObserver> {
        Arc::clone(&self.external_metrics)
    }

    pub(super) async fn check_ownership(&self, cookie: &EhentaiCookie) -> Result<(), ()> {
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
