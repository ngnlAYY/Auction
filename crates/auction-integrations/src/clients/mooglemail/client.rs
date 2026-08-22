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

    /// 对 typed target 执行一次受限 GET。
    ///
    /// 调用方必须在完成详情解析后立即丢弃响应正文；本方法不会把正文放进错误对象或日志
    /// 字段。Cookie 仅在建立请求时短暂复制到 HTTP header。
    pub async fn get(
        &self,
        target: &MoogleMailTarget,
        cookie: &MoogleMailCookie,
    ) -> Result<Vec<u8>, MoogleMailError> {
        if target.origin != self.builder.origin {
            return Err(self.configuration(target));
        }
        self.gate.wait().await.map_err(|_| {
            MoogleMailError::new(
                target.operation,
                target.world,
                MoogleMailErrorCategory::RemoteTemporary,
                true,
            )
        })?;

        let request = self
            .client
            .get(target.url.clone())
            .header(header::USER_AGENT, USER_AGENT)
            .header(header::COOKIE, cookie.expose())
            .timeout(self.request_timeout)
            .build()
            .map_err(|_| self.configuration(target))?;
        let response = self.client.execute(request).await.map_err(|_| {
            MoogleMailError::new(
                target.operation,
                target.world,
                MoogleMailErrorCategory::RemoteTemporary,
                true,
            )
        })?;

        if response.url().as_str() != target.url.as_str() {
            return Err(self.protocol(target));
        }
        if response
            .headers()
            .get(header::CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                !value.trim().is_empty() && !value.eq_ignore_ascii_case("identity")
            })
            || response
                .content_length()
                .is_some_and(|length| length > self.maximum_response_bytes as u64)
        {
            return Err(self.response_too_large(target));
        }
        let status = response.status();
        let location = response.headers().get(header::LOCATION).cloned();
        let is_html = is_html_content_type(response.headers());

        let body = time::timeout(self.request_timeout, response.bytes())
            .await
            .map_err(|_| {
                MoogleMailError::new(
                    target.operation,
                    target.world,
                    MoogleMailErrorCategory::RemoteTemporary,
                    true,
                )
            })?
            .map_err(|_| {
                MoogleMailError::new(
                    target.operation,
                    target.world,
                    MoogleMailErrorCategory::RemoteTemporary,
                    true,
                )
            })?;
        if body.len() > self.maximum_response_bytes {
            return Err(self.response_too_large(target));
        }

        if is_session_expired(status, location.as_ref(), &body) {
            return Err(MoogleMailError::new(
                target.operation,
                target.world,
                MoogleMailErrorCategory::SessionExpired,
                false,
            ));
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(MoogleMailError::new(
                target.operation,
                target.world,
                MoogleMailErrorCategory::RateLimited,
                true,
            ));
        }
        if status.is_server_error() {
            return Err(MoogleMailError::new(
                target.operation,
                target.world,
                MoogleMailErrorCategory::RemoteTemporary,
                true,
            ));
        }
        if !status.is_success() || !is_html {
            return Err(self.protocol(target));
        }
        Ok(body.to_vec())
    }

    /// 对 compose/detail typed target 执行恰好一次表单 POST。
    ///
    /// 请求开始后，无论是网络错误、响应结构错误还是远端明确拒绝，都不能证明远端没有
    /// 发生变化，因此统一返回 write_unknown，由上层进入人工对账或基于页面证明继续。
    pub async fn post(
        &self,
        target: &MoogleMailTarget,
        cookie: &MoogleMailCookie,
        form: MoogleMailForm,
    ) -> Result<Vec<u8>, MoogleMailError> {
        let operation = form.operation();
        if target.origin != self.builder.origin || !post_allowed(target.operation, operation) {
            return Err(self.configuration_for(operation, target.world));
        }
        self.gate.wait().await.map_err(|_| {
            MoogleMailError::new(
                operation,
                target.world,
                MoogleMailErrorCategory::RemoteTemporary,
                true,
            )
        })?;

        let encoded = SensitiveBytes(form.encode());
        let request = self
            .client
            .post(target.url.clone())
            .header(header::USER_AGENT, USER_AGENT)
            .header(header::COOKIE, cookie.expose())
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .timeout(self.request_timeout)
            .body(encoded.0.clone())
            .build()
            .map_err(|_| self.configuration_for(operation, target.world))?;
        let response = self
            .client
            .execute(request)
            .await
            .map_err(|_| self.write_unknown(operation, target.world))?;

        if response.url().as_str() != target.url.as_str() {
            return Err(self.write_unknown(operation, target.world));
        }
        if response
            .headers()
            .get(header::CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                !value.trim().is_empty() && !value.eq_ignore_ascii_case("identity")
            })
            || response
                .content_length()
                .is_some_and(|length| length > self.maximum_response_bytes as u64)
        {
            return Err(self.write_unknown(operation, target.world));
        }
        let status = response.status();
        let location = response.headers().get(header::LOCATION).cloned();
        let is_html = is_html_content_type(response.headers());
        let body = time::timeout(self.request_timeout, response.bytes())
            .await
            .map_err(|_| self.write_unknown(operation, target.world))?
            .map_err(|_| self.write_unknown(operation, target.world))?;
        if body.len() > self.maximum_response_bytes {
            return Err(self.write_unknown(operation, target.world));
        }
        if is_session_expired(status, location.as_ref(), &body)
            || status == StatusCode::TOO_MANY_REQUESTS
            || status.is_server_error()
            || !status.is_success()
            || !is_html
        {
            return Err(self.write_unknown(operation, target.world));
        }
        Ok(body.to_vec())
    }

    fn configuration(&self, target: &MoogleMailTarget) -> MoogleMailError {
        MoogleMailError::new(
            target.operation,
            target.world,
            MoogleMailErrorCategory::Configuration,
            false,
        )
    }

    fn configuration_for(
        &self,
        operation: MoogleMailOperation,
        world: MoogleMailWorld,
    ) -> MoogleMailError {
        MoogleMailError::new(operation, world, MoogleMailErrorCategory::Configuration, false)
    }

    fn write_unknown(
        &self,
        operation: MoogleMailOperation,
        world: MoogleMailWorld,
    ) -> MoogleMailError {
        MoogleMailError::new(operation, world, MoogleMailErrorCategory::WriteUnknown, false)
    }

    fn protocol(&self, target: &MoogleMailTarget) -> MoogleMailError {
        MoogleMailError::new(
            target.operation,
            target.world,
            MoogleMailErrorCategory::ProtocolDrift,
            false,
        )
    }

    fn response_too_large(&self, target: &MoogleMailTarget) -> MoogleMailError {
        MoogleMailError::new(
            target.operation,
            target.world,
            MoogleMailErrorCategory::ResponseTooLarge,
            false,
        )
    }
}

/// 构造不跟随重定向、关闭环境代理的 MoogleMail HTTP client。
fn build_mooglemail_http_client() -> Result<Client, reqwest::Error> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build()
}

fn is_html_content_type(headers: &header::HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| matches!(value.trim(), "text/html" | "application/xhtml+xml"))
}

fn is_session_expired(
    status: StatusCode,
    location: Option<&header::HeaderValue>,
    body: &[u8],
) -> bool {
    if status.is_redirection() {
        let location = location.and_then(|value| value.to_str().ok()).unwrap_or_default();
        let location = location.to_ascii_lowercase();
        if location.contains("bounce_login") || location.contains("e-hentai.org") {
            return true;
        }
    }
    let body = String::from_utf8_lossy(body).to_ascii_lowercase();
    body.contains("bounce_login.php")
        || body.contains("you are not logged in")
        || body.contains("this page requires you to log on")
}

struct IntervalGate {
    interval: Duration,
    next: Mutex<Instant>,
}

fn post_allowed(
    target_operation: MoogleMailOperation,
    form_operation: MoogleMailOperation,
) -> bool {
    match target_operation {
        MoogleMailOperation::Compose => matches!(
            form_operation,
            MoogleMailOperation::AttachAdd
                | MoogleMailOperation::AttachCod
                | MoogleMailOperation::Send
        ),
        MoogleMailOperation::Detail => {
            matches!(form_operation, MoogleMailOperation::ReturnMessage | MoogleMailOperation::Take)
        }
        _ => false,
    }
}

struct SensitiveBytes(Vec<u8>);

impl Drop for SensitiveBytes {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

impl IntervalGate {
    fn new(interval: Duration) -> Self {
        Self { interval, next: Mutex::new(Instant::now()) }
    }

    async fn wait(&self) -> Result<(), ()> {
        let delay = {
            let mut next = self.next.lock().map_err(|_| ())?;
            let now = Instant::now();
            let reserved = (*next).max(now);
            *next = reserved.checked_add(self.interval).ok_or(())?;
            reserved.saturating_duration_since(now)
        };
        if !delay.is_zero() {
            time::sleep(delay).await;
        }
        Ok(())
    }
}
