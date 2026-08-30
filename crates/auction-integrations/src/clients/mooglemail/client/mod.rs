use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use reqwest::{Client, StatusCode, header};
use tokio::time;
use url::Url;

use super::{
    MoogleMailError, MoogleMailErrorCategory, MoogleMailOperation, MoogleMailTarget,
    MoogleMailWorld,
    protocol::{MoogleMailCookie, MoogleMailEquipmentKey},
};
use super::{forms::MoogleMailForm, protocol::MoogleMailUrlBuilder};

mod request;
mod security;

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
    /// 连续请求之间的最小间隔。
    pub minimum_request_interval: Duration,
    /// 单次 HTTP 请求超时时间。
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
    maximum_response_bytes: usize,
    request_timeout: Duration,
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
            maximum_response_bytes,
            request_timeout: config.request_timeout,
        })
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

    /// 构造收件箱页目标。
    pub fn inbox_page(
        &self,
        world: MoogleMailWorld,
        page: usize,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        self.builder.inbox_page(world, page)
    }

    /// 构造单封收件箱详情目标。
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

    /// 构造已发送邮件详情目标。
    pub fn sent_detail(
        &self,
        world: MoogleMailWorld,
        external_id: &str,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        self.builder.sent_detail(world, external_id)
    }

    /// 构造新邮件 compose 目标。
    pub fn compose(&self, world: MoogleMailWorld) -> Result<MoogleMailTarget, MoogleMailError> {
        self.builder.compose(world)
    }

    /// 构造 Item Shop 余额目标。
    pub fn balance(&self, world: MoogleMailWorld) -> Result<MoogleMailTarget, MoogleMailError> {
        self.builder.balance(world)
    }

    /// 构造装备详情目标。
    pub fn equipment(
        &self,
        world: MoogleMailWorld,
        equipment_id: &str,
        equipment_key: &MoogleMailEquipmentKey,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        self.builder.equipment(world, equipment_id, equipment_key)
    }
}
