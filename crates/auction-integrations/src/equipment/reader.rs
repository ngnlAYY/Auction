//! 装备来源读取器与应用端口适配。
//!
//! 这里负责请求节流、无重定向读取、响应边界、world 路径选择和短生命周期 key 交接；页面
//! 事实解析与 canonical JSON 校验分别由 `parser`/`model` 拥有。

use auction_application::{
    AdminIntakeError, AdminIntakeSourceReader, AdminIntakeSourceSnapshot, ApplicationError,
    ApplicationFuture, CanonicalEquipment, EquipmentKeySealerPort, MailEquipmentAttachment,
    MailEquipmentReader,
};
use auction_domain::{EquipmentId, EquipmentKey};
use chrono::Utc;
use reqwest::{Client, StatusCode};
use tokio::{
    sync::Mutex,
    time::{Instant, sleep_until, timeout},
};
use url::Url;

use crate::{EquipmentKeySealer, EquipmentKeySealerError};

use super::{
    config::{DEFAULT_USER_AGENT, EquipmentSourceConfigurationError, EquipmentSourcePolicy},
    model::{canonical_facts, canonical_fingerprint, sha256},
    parser::{parse_equipment_page, parse_equipment_url},
};

/// canonical JSON 与旧装备页共用的来源适配器。
pub struct EquipmentSourceAdapter {
    client: Client,
    endpoint: Url,
    policy: EquipmentSourcePolicy,
    next_request_at: Mutex<Instant>,
}

impl EquipmentSourceAdapter {
    /// 建立固定 endpoint 的来源适配器。
    ///
    /// 传入的 client 应由装配根使用 `build_equipment_http_client` 创建，从而关闭自动重定向
    /// 和环境代理；适配器仍会再次检查最终响应 URL。
    pub fn new(
        client: Client,
        endpoint: Url,
        policy: EquipmentSourcePolicy,
    ) -> Result<Self, EquipmentSourceConfigurationError> {
        if endpoint.scheme() != "https"
            || endpoint.host_str().is_none()
            || endpoint.username() != ""
            || endpoint.password().is_some()
            || !matches!(endpoint.path(), "" | "/")
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(EquipmentSourceConfigurationError::InvalidEndpoint);
        }
        policy.validate()?;
        Ok(Self { client, endpoint, policy, next_request_at: Mutex::new(Instant::now()) })
    }

    /// 使用旧版生产 endpoint 创建适配器。
    pub fn production(
        client: Client,
        policy: EquipmentSourcePolicy,
    ) -> Result<Self, EquipmentSourceConfigurationError> {
        let endpoint = Url::parse("https://hentaiverse.org")
            .map_err(|_| EquipmentSourceConfigurationError::InvalidEndpoint)?;
        Self::new(client, endpoint, policy)
    }

    async fn read_url_inner(
        &self,
        world_id: String,
        equipment_url: String,
    ) -> Result<AdminIntakeSourceSnapshot, AdminIntakeError> {
        let (target, equipment_id, key) =
            parse_equipment_url(&self.endpoint, &self.policy, &world_id, &equipment_url)?;
        self.await_request_slot().await;
        let body = timeout(self.policy.request_timeout, self.fetch_document(target))
            .await
            .map_err(|_| AdminIntakeError::SourceUnavailable)??;
        let facts = parse_equipment_page(&body, &equipment_id)?;
        let key = EquipmentKey::new(&key).map_err(|_| AdminIntakeError::InvalidSource)?;
        let attributes = facts.attributes()?;
        let mut fingerprint_input = Vec::new();
        fingerprint_input.extend_from_slice(b"hentaiverse-equipment-v1\0");
        fingerprint_input.extend_from_slice(world_id.as_bytes());
        fingerprint_input.push(0);
        fingerprint_input.extend_from_slice(equipment_id.as_bytes());
        let source_fingerprint = sha256(fingerprint_input);
        Ok(AdminIntakeSourceSnapshot {
            adapter: "hentaiverse-equipment-v1".to_owned(),
            source_fingerprint,
            external_equipment_id: equipment_id,
            name: facts.name,
            category: facts.category,
            level: facts.level,
            tradeable: facts.tradeable,
            attributes,
            collected_at: Utc::now(),
            key,
        })
    }

    async fn await_request_slot(&self) {
        let mut next_request_at = self.next_request_at.lock().await;
        let now = Instant::now();
        if *next_request_at > now {
            sleep_until(*next_request_at).await;
        }
        *next_request_at = Instant::now() + self.policy.minimum_request_interval;
    }

    async fn fetch_document(&self, target: Url) -> Result<Vec<u8>, AdminIntakeError> {
        let mut response = self
            .client
            .get(target)
            .header(reqwest::header::USER_AGENT, DEFAULT_USER_AGENT)
            .send()
            .await
            .map_err(|_| AdminIntakeError::SourceUnavailable)?;
        if response.url().scheme() != self.endpoint.scheme()
            || response.url().host_str() != self.endpoint.host_str()
            || response.url().port_or_known_default() != self.endpoint.port_or_known_default()
        {
            return Err(AdminIntakeError::InvalidSource);
        }
        if response.status() == StatusCode::TOO_MANY_REQUESTS || response.status().is_server_error()
        {
            return Err(AdminIntakeError::SourceUnavailable);
        }
        if !response.status().is_success() {
            return Err(AdminIntakeError::InvalidSource);
        }
        if response
            .content_length()
            .is_some_and(|length| length > self.policy.maximum_response_bytes as u64)
        {
            return Err(AdminIntakeError::InvalidSource);
        }
        let content_encoding = response
            .headers()
            .get(reqwest::header::CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .trim();
        if !content_encoding.is_empty() && !content_encoding.eq_ignore_ascii_case("identity") {
            return Err(AdminIntakeError::InvalidSource);
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .split(';')
            .next()
            .map(str::trim)
            .unwrap_or_default();
        if !matches!(content_type, "text/html" | "application/xhtml+xml") {
            return Err(AdminIntakeError::InvalidSource);
        }
        let mut body = Vec::with_capacity(
            response
                .content_length()
                .and_then(|length| usize::try_from(length).ok())
                .unwrap_or_default()
                .min(self.policy.maximum_response_bytes),
        );
        while let Some(chunk) =
            response.chunk().await.map_err(|_| AdminIntakeError::SourceUnavailable)?
        {
            let next_length =
                body.len().checked_add(chunk.len()).ok_or(AdminIntakeError::InvalidSource)?;
            if next_length > self.policy.maximum_response_bytes {
                return Err(AdminIntakeError::InvalidSource);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    async fn read_mail_attachment_inner(
        &self,
        world_id: String,
        attachment: MailEquipmentAttachment,
    ) -> Result<AdminIntakeSourceSnapshot, AdminIntakeError> {
        let key = attachment
            .key
            .with_bytes(|bytes| std::str::from_utf8(bytes).map(str::to_owned))
            .map_err(|_| AdminIntakeError::InvalidSource)?
            .map_err(|_| AdminIntakeError::InvalidSource)?;
        let prefix = if world_id == "isekai" { "/isekai/equip/" } else { "/equip/" };
        let mut target = self.endpoint.clone();
        target.set_path(&format!("{prefix}{}/{key}", attachment.external_equipment_id));
        self.read_url_inner(world_id, target.to_string()).await
    }

    async fn read_json_inner(
        &self,
        world_id: String,
        input: CanonicalEquipment,
    ) -> Result<AdminIntakeSourceSnapshot, AdminIntakeError> {
        if !matches!(world_id.as_str(), "main" | "isekai") {
            return Err(AdminIntakeError::InvalidCommand);
        }
        input.validate()?;
        let facts = canonical_facts(&input)?;
        let key = EquipmentKey::new(&input.key).map_err(|_| AdminIntakeError::InvalidSource)?;
        let fingerprint = canonical_fingerprint(&world_id, &facts)?;
        let attributes = facts.attributes()?;
        Ok(AdminIntakeSourceSnapshot {
            adapter: "manual-json-v1".to_owned(),
            source_fingerprint: fingerprint,
            external_equipment_id: facts.external_equipment_id,
            name: facts.name,
            category: facts.category,
            level: facts.level,
            tradeable: facts.tradeable,
            attributes,
            collected_at: Utc::now(),
            key,
        })
    }
}

impl AdminIntakeSourceReader for EquipmentSourceAdapter {
    fn read_url(
        &self,
        world_id: String,
        equipment_url: String,
    ) -> ApplicationFuture<'_, AdminIntakeSourceSnapshot> {
        Box::pin(async move {
            self.read_url_inner(world_id, equipment_url).await.map_err(ApplicationError::from)
        })
    }

    fn read_json(
        &self,
        world_id: String,
        equipment: CanonicalEquipment,
    ) -> ApplicationFuture<'_, AdminIntakeSourceSnapshot> {
        Box::pin(async move {
            self.read_json_inner(world_id, equipment).await.map_err(ApplicationError::from)
        })
    }
}

impl MailEquipmentReader for EquipmentSourceAdapter {
    fn read(
        &self,
        world_id: String,
        attachment: MailEquipmentAttachment,
    ) -> ApplicationFuture<'_, AdminIntakeSourceSnapshot> {
        Box::pin(async move {
            self.read_mail_attachment_inner(world_id, attachment)
                .await
                .map_err(ApplicationError::from)
        })
    }
}

impl EquipmentKeySealerPort for EquipmentKeySealer {
    fn seal(
        &self,
        equipment_id: EquipmentId,
        key: &EquipmentKey,
    ) -> Result<auction_domain::EquipmentKeyEnvelope, AdminIntakeError> {
        EquipmentKeySealer::seal(self, equipment_id, key)
            .map_err(|_: EquipmentKeySealerError| AdminIntakeError::KeyUnavailable)
    }
}

/// 创建关闭代理和重定向的来源 HTTP client。
pub fn build_equipment_http_client() -> Result<Client, reqwest::Error> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build()
}
