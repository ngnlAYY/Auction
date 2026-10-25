use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use auction_application::{AdminCredentialRuntimeOwnership, MailDiagnosticRecorder};
use reqwest::{Client, StatusCode, header};
use tokio::{sync::Mutex, time};
use url::Url;

use super::{
    MoogleMailError, MoogleMailErrorCategory, MoogleMailOperation, MoogleMailTarget,
    MoogleMailWorld,
    protocol::{MoogleMailCookie, MoogleMailEquipmentKey},
};
use super::{forms::MoogleMailForm, protocol::MoogleMailUrlBuilder};

mod diagnostics;
mod reads;
mod request;
mod security;

pub(crate) use reads::MoogleMailReadRound;
pub(crate) use security::is_hentaiverse_login_response;
use security::{IntervalGate, build_mooglemail_http_client};

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
    gate: Arc<IntervalGate>,
    reads: Arc<reads::MailboxReads>,
    maximum_response_bytes: usize,
    request_timeout: Duration,
    diagnostics: Option<Arc<dyn MailDiagnosticRecorder>>,
    ownership: Option<Arc<dyn AdminCredentialRuntimeOwnership>>,
}

impl MoogleMailClient {
    /// 使用配置构造适配器，并强制采用关闭代理和自动重定向的 HTTP client。
    pub fn with_config(config: MoogleMailClientConfig) -> Result<Self, MoogleMailError> {
        let client = build_mooglemail_http_client().map_err(|_| {
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
        let config = MoogleMailClientConfig::production().map_err(|_| {
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
            gate: Arc::new(IntervalGate::new(config.minimum_request_interval)),
            reads: Arc::new(reads::MailboxReads::default()),
            maximum_response_bytes,
            request_timeout: config.request_timeout,
            diagnostics: None,
            ownership: None,
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
                .map_err(|_| ())?
                .map_err(|_| ())?;
        }
        Ok(())
    }

    /// 注入异常证据记录器；诊断失败不能改变远端结果分类或触发额外请求。
    pub fn with_diagnostics(mut self, diagnostics: Arc<dyn MailDiagnosticRecorder>) -> Self {
        self.diagnostics = Some(diagnostics);
        self
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
