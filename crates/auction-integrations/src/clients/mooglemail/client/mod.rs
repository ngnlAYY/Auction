use auction_application::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use auction_application::{AdminCredentialRuntimeOwnership, MailDiagnosticRecorder};
use reqwest::{Client, StatusCode, header};
use tokio::time;
use url::Url;

use super::{
    MoogleMailError, MoogleMailErrorCategory, MoogleMailOperation, MoogleMailTarget,
    MoogleMailWorld,
    protocol::{MoogleMailCookie, MoogleMailEquipmentKey},
};
use super::{forms::MoogleMailForm, protocol::MoogleMailUrlBuilder};

mod account_busy;
mod diagnostics;
mod reads;
mod request;
mod security;

use crate::clients::gate::PriorityGate;
pub(crate) use reads::{
    MoogleMailIncomingList, MoogleMailIncomingPrefix, MoogleMailReadRound, MoogleMailSentList,
    MoogleMailSentPrefix, MoogleMailSentScope, SentBoundary, SentBoundaryScan,
};
use security::build_mooglemail_http_client;
pub(crate) use security::is_hentaiverse_login_response;

const DEFAULT_MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_MINIMUM_REQUEST_INTERVAL: Duration = Duration::from_millis(750);
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";

/// MoogleMail HTTP 客户端的硬边界。
#[derive(Debug, Clone)]
pub struct MoogleMailClientConfig {
    /// 固定的生产 endpoint 或显式回环测试 endpoint。
    pub endpoint: Url,
    /// 单次响应最大字节数，不能超过 8 MiB。
    pub maximum_response_bytes: usize,
    pub minimum_request_interval: Duration,
    pub request_timeout: Duration,
}

impl MoogleMailClientConfig {
    /// 返回生产 HentaiVerse endpoint 的默认配置。
    pub fn production() -> Result<Self, url::ParseError> {
        Ok(Self {
            endpoint: Url::parse("https://hentaiverse.org")?,
            maximum_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            minimum_request_interval: DEFAULT_MINIMUM_REQUEST_INTERVAL,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        })
    }
}

/// 共享的 MoogleMail typed HTTP client。
#[derive(Clone)]
pub struct MoogleMailClient {
    builder: MoogleMailUrlBuilder,
    client: Client,
    gate: Arc<PriorityGate>,
    /// 账号忙熔断只限制请求节奏；克隆出的客户端共享同一份状态。
    account_busy: Arc<account_busy::MoogleMailAccountBusy>,
    reads: Arc<reads::MailboxReads>,
    maximum_response_bytes: usize,
    request_timeout: Duration,
    /// Sent / Inbox / Read Mail 水位每日完整校验的最大间隔；由 Worker 部署配置注入，
    /// 只决定何时多读一轮完整列表，不改变任何水位、证明或发送结果。
    mail_scan_daily_full_interval: chrono::Duration,
    diagnostics: Option<Arc<dyn MailDiagnosticRecorder>>,
    ownership: Option<Arc<dyn AdminCredentialRuntimeOwnership>>,
    external_metrics: Arc<dyn auction_application::ExternalMetricsObserver>,
}

impl MoogleMailClient {
    /// 使用配置构造适配器，并强制采用关闭代理和自动重定向的 HTTP client。
    pub fn with_config(config: MoogleMailClientConfig) -> Result<Self, MoogleMailError> {
        let client = build_mooglemail_http_client().map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "build_http_client",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MoogleMailError::new(
                MoogleMailOperation::Inbox,
                MoogleMailWorld::Main,
                MoogleMailErrorCategory::Configuration,
                false,
            )
        })?;
        Self::from_config_and_client(config, client)
    }

    /// 使用生产固定 endpoint 和默认限速/超时构造适配器。
    pub fn production() -> Result<Self, MoogleMailError> {
        let config = MoogleMailClientConfig::production().map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "validate_production_configuration",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MoogleMailError::new(
                MoogleMailOperation::Inbox,
                MoogleMailWorld::Main,
                MoogleMailErrorCategory::Configuration,
                false,
            )
        })?;
        Self::with_config(config)
    }

    fn from_config_and_client(
        config: MoogleMailClientConfig,
        client: Client,
    ) -> Result<Self, MoogleMailError> {
        let maximum_response_bytes = if config.maximum_response_bytes == 0 {
            DEFAULT_MAX_RESPONSE_BYTES
        } else {
            config.maximum_response_bytes
        };
        if maximum_response_bytes == 0
            || maximum_response_bytes > DEFAULT_MAX_RESPONSE_BYTES
            || config.minimum_request_interval.is_zero()
            || config.request_timeout.is_zero()
        {
            return Err(MoogleMailError::new(
                MoogleMailOperation::Inbox,
                MoogleMailWorld::Main,
                MoogleMailErrorCategory::Configuration,
                false,
            ));
        }
        let builder = MoogleMailUrlBuilder::new(config.endpoint)?;
        Ok(Self {
            builder,
            client,
            gate: Arc::new(PriorityGate::new(config.minimum_request_interval)),
            account_busy: Arc::new(account_busy::MoogleMailAccountBusy::default()),
            reads: Arc::new(reads::MailboxReads::default()),
            maximum_response_bytes,
            request_timeout: config.request_timeout,
            mail_scan_daily_full_interval: auction_application::MAIL_SCAN_DAILY_FULL_INTERVAL,
            diagnostics: None,
            ownership: None,
            external_metrics: auction_application::noop_external_metrics_observer(),
        })
    }

    /// 由凭据集合继承持锁 owner；所有客户端副本共享同一个永久失效状态。
    pub(crate) fn with_ownership(
        mut self,
        ownership: Option<Arc<dyn AdminCredentialRuntimeOwnership>>,
    ) -> Self {
        if ownership.is_some() {
            self.ownership = ownership;
        }
        self
    }

    async fn check_ownership(&self) -> Result<(), ()> {
        if let Some(ownership) = &self.ownership {
            // 预算同时覆盖持锁连接的排队与探测；超时取消会永久撤销 owner。
            time::timeout(Duration::from_secs(1), ownership.check())
                .await
                .map_err(|diagnostic_cause| {
                    safe_diagnostics::warn(
                        "mooglemail_session",
                        "check_owner_timeout",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                })?
                .map_err(|diagnostic_cause| {
                    safe_diagnostics::warn(
                        "mooglemail_session",
                        "check_owner",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                })?;
        }
        Ok(())
    }

    /// 注入异常证据记录器；诊断失败不能改变远端结果分类或触发额外请求。
    pub fn with_diagnostics(mut self, diagnostics: Arc<dyn MailDiagnosticRecorder>) -> Self {
        self.diagnostics = Some(diagnostics);
        self
    }

    /// 注入同步外部客户端指标；只读取共享门的配置间隔，不改变实际间隔。
    pub fn with_external_metrics(
        mut self,
        observer: Arc<dyn auction_application::ExternalMetricsObserver>,
    ) -> Self {
        observer.observe(auction_application::ExternalMetricsEvent::MailRequestInterval {
            interval: self.gate.interval(),
        });
        self.external_metrics = observer;
        self
    }

    /// 注入部署配置的每日完整校验间隔；零值不生效，保持 24 小时默认。
    /// 水位、游标与证明语义不变，只影响多读一轮完整列表的节奏。
    pub fn with_mail_scan_daily_full_interval(mut self, interval: chrono::Duration) -> Self {
        if !interval.is_zero() {
            self.mail_scan_daily_full_interval = interval;
        }
        self
    }

    /// 每日完整校验间隔；供 Sent/Inbox/Read Mail 的扫描计划共享同一取值。
    pub(crate) fn mail_scan_daily_full_interval(&self) -> chrono::Duration {
        self.mail_scan_daily_full_interval
    }

    /// 供 sender 的扫描预算 owner 记录同步指标；不暴露给进程外调用方。
    pub(crate) fn external_metrics(&self) -> Arc<dyn auction_application::ExternalMetricsObserver> {
        Arc::clone(&self.external_metrics)
    }

    /// 远端 endpoint 的稳定 origin；只用于扫描检查点作用域，不含路径、凭据或查询。
    pub(crate) fn endpoint_scope(&self) -> &str {
        &self.builder.origin
    }

    /// 账号忙熔断窗口内不允许发起远端写入；失败关闭的断言在请求发出前后仍由适配器执行。
    /// 调用方只能在 sending 围栏前使用该判断，不能据此改写业务状态。
    pub fn permits_write(&self, world: MoogleMailWorld) -> bool {
        self.account_busy.permits_write(world, Instant::now())
    }

    /// 让同一客户端的 Worker 业务处理复用诊断 owner，避免并发写入绕过容量边界。
    pub fn diagnostic_recorder(&self) -> Option<Arc<dyn MailDiagnosticRecorder>> {
        self.diagnostics.clone()
    }

    /// 使用指定固定 endpoint 和默认限速/超时构造适配器。
    pub fn new(base_url: Url) -> Result<Self, MoogleMailError> {
        Self::with_config(MoogleMailClientConfig {
            endpoint: base_url,
            maximum_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            minimum_request_interval: DEFAULT_MINIMUM_REQUEST_INTERVAL,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        })
    }

    pub fn inbox_page(
        &self,
        world: MoogleMailWorld,
        page: usize,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        self.builder.inbox_page(world, page)
    }

    /// 已读历史列表，不作为新入站指令来源。
    pub fn read_mail_page(
        &self,
        world: MoogleMailWorld,
        page: usize,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        self.builder.read_mail_page(world, page)
    }

    pub fn detail(
        &self,
        world: MoogleMailWorld,
        external_id: &str,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        self.builder.detail(world, external_id)
    }

    /// 构造已发送邮件页目标，供后续回执读取使用。
    pub fn sent_page(
        &self,
        world: MoogleMailWorld,
        page: usize,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        self.builder.sent_page(world, page)
    }

    pub fn sent_detail(
        &self,
        world: MoogleMailWorld,
        external_id: &str,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        self.builder.sent_detail(world, external_id)
    }

    pub fn compose(&self, world: MoogleMailWorld) -> Result<MoogleMailTarget, MoogleMailError> {
        self.builder.compose(world)
    }

    pub fn balance(&self, world: MoogleMailWorld) -> Result<MoogleMailTarget, MoogleMailError> {
        self.builder.balance(world)
    }

    /// 构造认证自身 UID 的只读固定目标。
    pub fn identity(&self, world: MoogleMailWorld) -> MoogleMailTarget {
        self.builder.identity(world)
    }

    /// 构造闭集分类的 Armory organize 只读目标。
    pub fn inventory(
        &self,
        world: MoogleMailWorld,
        category: super::protocol::MoogleMailInventoryCategory,
    ) -> MoogleMailTarget {
        self.builder.inventory(world, category)
    }

    /// 构造同源、同世界的装备名称脚本目标，供现有 typed GET 读取。
    pub(crate) fn inventory_metadata(
        &self,
        world: MoogleMailWorld,
        source: &str,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        self.builder.inventory_metadata(world, source)
    }

    /// 装备发送准备专用的固定 Armory 写入目标。
    pub(crate) fn inventory_organize(&self, world: MoogleMailWorld) -> MoogleMailTarget {
        self.builder.inventory_organize(world)
    }

    pub fn equipment(
        &self,
        world: MoogleMailWorld,
        equipment_id: &str,
        equipment_key: &MoogleMailEquipmentKey,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        self.builder.equipment(world, equipment_id, equipment_key)
    }
}

#[cfg(test)]
mod tests {
    use super::{MoogleMailClient, MoogleMailClientConfig};

    #[test]
    fn daily_full_interval_defaults_to_one_day_and_accepts_configured_value() {
        let config = MoogleMailClientConfig::production().expect("生产 endpoint 可解析");
        let client = MoogleMailClient::with_config(config).expect("默认客户端可构造");
        assert_eq!(
            client.mail_scan_daily_full_interval(),
            auction_application::MAIL_SCAN_DAILY_FULL_INTERVAL
        );

        let client = client.with_mail_scan_daily_full_interval(chrono::Duration::hours(1));
        assert_eq!(client.mail_scan_daily_full_interval(), chrono::Duration::hours(1));

        // 零值不生效：配置校验已拒绝 0，这里保持默认而不是退化成每轮完整扫描。
        let client = client.with_mail_scan_daily_full_interval(chrono::Duration::zero());
        assert_eq!(client.mail_scan_daily_full_interval(), chrono::Duration::hours(1));
    }
}
