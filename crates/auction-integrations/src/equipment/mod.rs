//! Admin 装备来源适配器。
//!
//! 这里把 canonical JSON 和旧版 Hentaiverse 装备页归一为同一份脱敏快照。URL 读取先经过
//! 启动时冻结的 host/path allowlist，再要求固定 endpoint origin、无重定向的 HTTPS 请求和
//! `/equip/{id}/{key}` 形状；共享适配器同时实施请求间隔、全请求超时和流式响应上限。解析失败
//! 时不把原始 HTML 交给应用层或数据库层。

use std::{collections::BTreeMap, fmt, time::Duration};

use auction_application::{
    AdminIntakeError, AdminIntakeSourceReader, AdminIntakeSourceSnapshot, ApplicationError,
    ApplicationFuture, CanonicalEquipment, EquipmentKeySealerPort, MailEquipmentAttachment,
    MailEquipmentReader,
};
use auction_domain::{EquipmentId, EquipmentKey};
use chrono::Utc;
use reqwest::{Client, StatusCode};
use scraper::{ElementRef, Html, Selector};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::{
    sync::Mutex,
    time::{Instant, sleep_until, timeout},
};
use url::Url;

use crate::{EquipmentKeySealer, EquipmentKeySealerError};

const DEFAULT_MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MINIMUM_MAX_RESPONSE_BYTES: usize = 64 * 1024;
const DEFAULT_USER_AGENT: &str = "auction-rust-equipment-source/1";

/// 装备来源适配器初始化错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EquipmentSourceConfigurationError {
    InvalidEndpoint,
    InvalidAllowlist,
    InvalidResponseLimit,
    InvalidRequestInterval,
    InvalidRequestTimeout,
}

impl fmt::Display for EquipmentSourceConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidEndpoint => "装备来源 endpoint 无效",
            Self::InvalidAllowlist => "装备来源 allowlist 无效",
            Self::InvalidResponseLimit => "装备来源响应大小限制无效",
            Self::InvalidRequestInterval => "装备来源请求间隔无效",
            Self::InvalidRequestTimeout => "装备来源请求超时无效",
        })
    }
}

impl std::error::Error for EquipmentSourceConfigurationError {}

/// 装备来源适配器在进程启动时冻结的非敏感读取策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EquipmentSourcePolicy {
    /// 允许访问的规范小写 DNS 主机名。
    pub allowed_hosts: Vec<String>,
    /// 允许访问的绝对路径前缀。
    pub allowed_path_prefixes: Vec<String>,
    /// 是否在固定 endpoint 与装备路径校验之外再启用业务 allowlist。
    pub allowlist_enabled: bool,
    /// 单个响应解码前允许读取的最大字节数。
    pub maximum_response_bytes: usize,
    /// 同一适配器两次远程请求起点之间的最小间隔。
    pub minimum_request_interval: Duration,
    /// 从发送请求到完整流式读取结束的总超时。
    pub request_timeout: Duration,
}

impl Default for EquipmentSourcePolicy {
    fn default() -> Self {
        Self {
            allowed_hosts: vec!["hentaiverse.org".to_owned()],
            allowed_path_prefixes: vec!["/equip/".to_owned(), "/isekai/equip/".to_owned()],
            allowlist_enabled: true,
            maximum_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            minimum_request_interval: Duration::from_secs(1),
            request_timeout: Duration::from_secs(10),
        }
    }
}

impl EquipmentSourcePolicy {
    fn validate(&self) -> Result<(), EquipmentSourceConfigurationError> {
        if !(MINIMUM_MAX_RESPONSE_BYTES..=DEFAULT_MAX_RESPONSE_BYTES)
            .contains(&self.maximum_response_bytes)
        {
            return Err(EquipmentSourceConfigurationError::InvalidResponseLimit);
        }
        if self.minimum_request_interval.is_zero()
            || self.minimum_request_interval > Duration::from_secs(60)
        {
            return Err(EquipmentSourceConfigurationError::InvalidRequestInterval);
        }
        if self.request_timeout.is_zero() || self.request_timeout > Duration::from_secs(30) {
            return Err(EquipmentSourceConfigurationError::InvalidRequestTimeout);
        }
        if self.allowed_hosts.len() > 16
            || self.allowed_path_prefixes.len() > 16
            || self.allowlist_enabled
                && (self.allowed_hosts.is_empty() || self.allowed_path_prefixes.is_empty())
            || self.allowed_hosts.iter().enumerate().any(|(index, host)| {
                !valid_dns_name(host) || self.allowed_hosts[..index].contains(host)
            })
            || self.allowed_path_prefixes.iter().enumerate().any(|(index, prefix)| {
                !valid_path_prefix(prefix) || self.allowed_path_prefixes[..index].contains(prefix)
            })
        {
            return Err(EquipmentSourceConfigurationError::InvalidAllowlist);
        }
        Ok(())
    }

    fn allows(&self, target: &Url) -> bool {
        !self.allowlist_enabled
            || target
                .host_str()
                .is_some_and(|host| self.allowed_hosts.iter().any(|item| item == host))
                && self.allowed_path_prefixes.iter().any(|prefix| target.path().starts_with(prefix))
    }
}

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
        let (target, equipment_id, key) = self.parse_equipment_url(&world_id, &equipment_url)?;
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

    fn parse_equipment_url(
        &self,
        world_id: &str,
        raw_url: &str,
    ) -> Result<(Url, String, String), AdminIntakeError> {
        if !matches!(world_id, "main" | "isekai")
            || raw_url.is_empty()
            || raw_url != raw_url.trim()
            || raw_url.len() > 2048
            || raw_url.contains('\\')
            || raw_url.chars().any(char::is_control)
        {
            return Err(AdminIntakeError::InvalidCommand);
        }
        let parsed = Url::parse(raw_url).map_err(|_| AdminIntakeError::InvalidSource)?;
        if parsed.scheme() != self.endpoint.scheme()
            || parsed.host_str() != self.endpoint.host_str()
            || parsed.port_or_known_default() != self.endpoint.port_or_known_default()
            || parsed.username() != ""
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.path().contains("..")
            || parsed.as_str() != raw_url
            || !self.policy.allows(&parsed)
        {
            return Err(AdminIntakeError::InvalidSource);
        }
        let prefix = if world_id == "isekai" { "/isekai/equip/" } else { "/equip/" };
        let parts: Vec<String> = parsed
            .path()
            .strip_prefix(prefix)
            .unwrap_or_default()
            .split('/')
            .map(str::to_owned)
            .collect();
        if parts.len() != 2 || !valid_decimal_id(&parts[0]) || EquipmentKey::new(&parts[1]).is_err()
        {
            return Err(AdminIntakeError::InvalidSource);
        }
        Ok((parsed, parts[0].clone(), parts[1].to_ascii_lowercase()))
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

struct EquipmentFacts {
    external_equipment_id: String,
    name: String,
    category: String,
    equipment_type: String,
    quality: String,
    prefix: String,
    suffix: String,
    level: Option<i32>,
    tradeable: bool,
    stats: BTreeMap<String, EquipmentStat>,
}

#[derive(Clone, Serialize)]
struct EquipmentStat {
    value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    base: Option<f64>,
}

impl EquipmentFacts {
    fn attributes(&self) -> Result<serde_json::Value, AdminIntakeError> {
        serde_json::to_value(serde_json::json!({
            "type": self.equipment_type,
            "quality": self.quality,
            "prefix": self.prefix,
            "suffix": self.suffix,
            "level": self.level,
            "tradeable": self.tradeable,
            "stats": self.stats,
        }))
        .map_err(|_| AdminIntakeError::InvalidSource)
    }
}

fn canonical_facts(input: &CanonicalEquipment) -> Result<EquipmentFacts, AdminIntakeError> {
    let (quality, prefix, equipment_type, suffix) = parse_identity(&input.name)?;
    if quality != input.quality
        || prefix != input.prefix
        || equipment_type != input.equipment_type
        || suffix != input.suffix
        || category_for_type(&input.equipment_type).as_deref() != Some(input.category.as_str())
    {
        return Err(AdminIntakeError::InvalidSource);
    }
    let stats = input
        .stats
        .iter()
        .map(|(name, stat)| {
            let value = normalize_stat_value(&stat.value).ok_or(AdminIntakeError::InvalidSource)?;
            if stat
                .base
                .is_some_and(|base| !base.is_finite() || !(0.0..=1_000_000.0).contains(&base))
            {
                return Err(AdminIntakeError::InvalidSource);
            }
            Ok((name.clone(), EquipmentStat { value, base: stat.base }))
        })
        .collect::<Result<BTreeMap<_, _>, AdminIntakeError>>()?;
    Ok(EquipmentFacts {
        external_equipment_id: input.external_equipment_id.clone(),
        name: input.name.clone(),
        category: input.category.clone(),
        equipment_type,
        quality,
        prefix,
        suffix,
        level: input.level,
        tradeable: input.tradeable,
        stats,
    })
}

#[derive(Serialize)]
struct CanonicalFingerprint<'a> {
    world: &'a str,
    external_equipment_id: &'a str,
    name: &'a str,
    category: &'a str,
    equipment_type: &'a str,
    quality: &'a str,
    prefix: &'a str,
    suffix: &'a str,
    level: Option<i32>,
    tradeable: bool,
    stats: &'a BTreeMap<String, EquipmentStat>,
}

fn canonical_fingerprint(
    world_id: &str,
    facts: &EquipmentFacts,
) -> Result<[u8; 32], AdminIntakeError> {
    let encoded = serde_json::to_vec(&CanonicalFingerprint {
        world: world_id,
        external_equipment_id: &facts.external_equipment_id,
        name: &facts.name,
        category: &facts.category,
        equipment_type: &facts.equipment_type,
        quality: &facts.quality,
        prefix: &facts.prefix,
        suffix: &facts.suffix,
        level: facts.level,
        tradeable: facts.tradeable,
        stats: &facts.stats,
    })
    .map_err(|_| AdminIntakeError::InvalidSource)?;
    let mut input = b"manual-json-v1\0".to_vec();
    input.extend_from_slice(&encoded);
    Ok(sha256(input))
}

fn parse_equipment_page(
    body: &[u8],
    external_equipment_id: &str,
) -> Result<EquipmentFacts, AdminIntakeError> {
    let document = std::str::from_utf8(body).map_err(|_| AdminIntakeError::InvalidSource)?;
    let document = Html::parse_document(document);
    let panel_selector = selector("div.showequip")?;
    let panels: Vec<ElementRef<'_>> = document.select(&panel_selector).collect();
    if panels.len() != 1 {
        return Err(AdminIntakeError::InvalidSource);
    }
    let panel = panels[0];
    let child_selector = selector("div.showequip > div")?;
    let children: Vec<ElementRef<'_>> = panel.select(&child_selector).collect();
    if children.len() < 2 {
        return Err(AdminIntakeError::InvalidSource);
    }
    let name = normalize_text(children[0].text().collect::<String>());
    if !valid_text(&name, 512) {
        return Err(AdminIntakeError::InvalidSource);
    }
    let header_selector = selector(".eqt")?;
    let header = children[1]
        .select(&header_selector)
        .next()
        .map(|value| normalize_text(value.text().collect::<String>()))
        .ok_or(AdminIntakeError::InvalidSource)?;
    let (category, level, tradeable) = parse_header(&header)?;
    let (quality, prefix, equipment_type, suffix) = parse_identity(&name)?;
    if category_for_type(&equipment_type).as_deref() != Some(category.as_str()) || !tradeable {
        return Err(AdminIntakeError::InvalidSource);
    }
    let stats = parse_stats(children[1])?;
    Ok(EquipmentFacts {
        external_equipment_id: external_equipment_id.to_owned(),
        name,
        category,
        equipment_type,
        quality,
        prefix,
        suffix,
        level,
        tradeable,
        stats,
    })
}

fn parse_header(value: &str) -> Result<(String, Option<i32>, bool), AdminIntakeError> {
    let mut fields: Vec<&str> = value.split_whitespace().collect();
    let tradeable = fields.last().is_some_and(|field| *field == "Tradeable");
    if fields.last().is_some_and(|field| *field == "Tradeable" || *field == "Soulbound") {
        fields.pop();
    }
    let mut level = None;
    if let Some(index) = fields.iter().position(|field| *field == "Level") {
        let raw = fields.get(index + 1).ok_or(AdminIntakeError::InvalidSource)?;
        let parsed = raw.parse::<i32>().map_err(|_| AdminIntakeError::InvalidSource)?;
        if !(1..=100_000).contains(&parsed) {
            return Err(AdminIntakeError::InvalidSource);
        }
        level = Some(parsed);
        fields.drain(index..=index + 1);
    }
    let category = normalize_category(&fields.join(" ")).ok_or(AdminIntakeError::InvalidSource)?;
    Ok((category, level, tradeable))
}

fn parse_identity(name: &str) -> Result<(String, String, String, String), AdminIntakeError> {
    let fields: Vec<&str> = name.split_whitespace().collect();
    if fields.len() < 2 {
        return Err(AdminIntakeError::InvalidSource);
    }
    let qualities = [
        "Peerless",
        "Legendary",
        "Magnificent",
        "Exquisite",
        "Superior",
        "Average",
        "Fair",
        "Crude",
    ];
    let quality = qualities
        .iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(fields[0]))
        .ok_or(AdminIntakeError::InvalidSource)?
        .to_string();
    let body = &fields[1..];
    let separator = body.iter().position(|field| field.eq_ignore_ascii_case("of"));
    let (type_fields, suffix) = match separator {
        Some(index) if index > 0 && index + 1 < body.len() => {
            (&body[..index], body[index + 1..].join(" "))
        }
        Some(_) => return Err(AdminIntakeError::InvalidSource),
        None => (body, String::new()),
    };
    let joined = type_fields.join(" ");
    let types = equipment_types();
    let mut candidates: Vec<&str> = types.iter().map(|(_, value)| *value).collect();
    candidates.sort_by_key(|value| std::cmp::Reverse(value.split_whitespace().count()));
    let mut selected = None;
    for candidate in candidates {
        if joined.eq_ignore_ascii_case(candidate) {
            selected = Some((candidate.to_owned(), String::new()));
            break;
        }
        if joined.len() > candidate.len()
            && joined
                .get(joined.len() - candidate.len()..)
                .is_some_and(|value| value.eq_ignore_ascii_case(candidate))
            && joined.as_bytes().get(joined.len() - candidate.len() - 1) == Some(&b' ')
        {
            let prefix = joined[..joined.len() - candidate.len() - 1].trim().to_owned();
            selected = Some((candidate.to_owned(), prefix));
            break;
        }
    }
    let (equipment_type, prefix) = selected.ok_or(AdminIntakeError::InvalidSource)?;
    if !valid_optional_text(&prefix, 128) || !valid_optional_text(&suffix, 128) {
        return Err(AdminIntakeError::InvalidSource);
    }
    Ok((quality, prefix, equipment_type, suffix))
}

fn parse_stats(panel: ElementRef<'_>) -> Result<BTreeMap<String, EquipmentStat>, AdminIntakeError> {
    let mut stats = BTreeMap::new();
    let title_selector = selector("[title]")?;
    for element in panel.select(&title_selector) {
        let Some(title) = element.value().attr("title") else { continue };
        let Some(base) = parse_base(title) else { continue };
        let text = normalize_text(element.text().collect::<String>());
        let (name, value) = parse_main_stat(&text).ok_or(AdminIntakeError::InvalidSource)?;
        insert_stat(&mut stats, name, value, Some(base))?;
    }
    let row_selector = selector(".ex > div, .ep > div")?;
    for row in panel.select(&row_selector) {
        let elements: Vec<ElementRef<'_>> = row.children().filter_map(ElementRef::wrap).collect();
        if elements.len() < 2 || elements.len() > 3 {
            continue;
        }
        let name = normalize_text(elements[0].text().collect::<String>());
        let value = normalize_stat_value(&normalize_text(elements[1].text().collect::<String>()))
            .ok_or(AdminIntakeError::InvalidSource)?;
        if elements.len() == 3 {
            let unit = normalize_text(elements[2].text().collect::<String>());
            if unit != "%" && unit != "x" {
                return Err(AdminIntakeError::InvalidSource);
            }
        }
        let base = row.value().attr("title").and_then(parse_base);
        insert_stat(&mut stats, name, value, base)?;
    }
    Ok(stats)
}

fn insert_stat(
    stats: &mut BTreeMap<String, EquipmentStat>,
    name: String,
    value: String,
    base: Option<f64>,
) -> Result<(), AdminIntakeError> {
    if !valid_text(&name, 128) || stats.insert(name, EquipmentStat { value, base }).is_some() {
        return Err(AdminIntakeError::InvalidSource);
    }
    if stats.len() > 256 {
        return Err(AdminIntakeError::InvalidSource);
    }
    Ok(())
}

fn parse_main_stat(value: &str) -> Option<(String, String)> {
    let mut fields = value.splitn(2, char::is_whitespace);
    let raw_value = fields.next()?;
    let name = normalize_text(fields.next()?.trim());
    let normalized = normalize_stat_value(raw_value)?;
    if !normalized.starts_with('+') || !valid_text(&name, 128) {
        return None;
    }
    Some((name, normalized))
}

fn parse_base(value: &str) -> Option<f64> {
    let value = value.trim().strip_prefix("Base:")?.trim();
    let parsed = value.parse::<f64>().ok()?;
    parsed.is_finite().then_some(parsed).filter(|value| (0.0..=1_000_000.0).contains(value))
}

fn normalize_stat_value(value: &str) -> Option<String> {
    let value = value.trim();
    let first = value.as_bytes().first().copied()?;
    if !matches!(first, b'+' | b'-') || value.len() == 1 {
        return None;
    }
    let mut dot_seen = false;
    let mut digits = 0;
    let mut normalized = String::with_capacity(value.len());
    normalized.push(first as char);
    for byte in value.as_bytes()[1..].iter().copied() {
        match byte {
            b'0'..=b'9' => {
                digits += 1;
                normalized.push(byte as char);
            }
            b',' if !dot_seen => {}
            b'.' if !dot_seen => {
                dot_seen = true;
                normalized.push('.');
            }
            _ => return None,
        }
    }
    (digits > 0).then_some(normalized)
}

fn normalize_category(value: &str) -> Option<String> {
    match value
        .to_ascii_lowercase()
        .replace(['-', '_'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .as_str()
    {
        "one handed weapon" => Some("One-Handed".to_owned()),
        "two handed weapon" => Some("Two-Handed".to_owned()),
        "staff" => Some("Staff".to_owned()),
        "shield" => Some("Shield".to_owned()),
        "cloth armor" => Some("Cloth".to_owned()),
        "light armor" => Some("Light".to_owned()),
        "heavy armor" => Some("Heavy".to_owned()),
        _ => None,
    }
}

fn category_for_type(value: &str) -> Option<String> {
    equipment_types()
        .into_iter()
        .find(|(_, candidate)| *candidate == value)
        .map(|(category, _)| category.to_owned())
}

fn equipment_types() -> Vec<(&'static str, &'static str)> {
    vec![
        ("One-Handed", "Axe"),
        ("One-Handed", "Club"),
        ("One-Handed", "Rapier"),
        ("One-Handed", "Shortsword"),
        ("One-Handed", "Wakizashi"),
        ("Two-Handed", "Estoc"),
        ("Two-Handed", "Longsword"),
        ("Two-Handed", "Mace"),
        ("Two-Handed", "Katana"),
        ("Staff", "Oak Staff"),
        ("Staff", "Willow Staff"),
        ("Staff", "Redwood Staff"),
        ("Staff", "Katalox Staff"),
        ("Shield", "Buckler"),
        ("Shield", "Kite Shield"),
        ("Shield", "Force Shield"),
        ("Cloth", "Cotton Cap"),
        ("Cloth", "Phase Cap"),
        ("Cloth", "Cotton Robe"),
        ("Cloth", "Phase Robe"),
        ("Cloth", "Cotton Gloves"),
        ("Cloth", "Phase Gloves"),
        ("Cloth", "Cotton Pants"),
        ("Cloth", "Phase Pants"),
        ("Cloth", "Cotton Shoes"),
        ("Cloth", "Phase Shoes"),
        ("Light", "Leather Cap"),
        ("Light", "Shade Helmet"),
        ("Light", "Leather Breastplate"),
        ("Light", "Shade Breastplate"),
        ("Light", "Leather Gauntlets"),
        ("Light", "Shade Gauntlets"),
        ("Light", "Leather Leggings"),
        ("Light", "Shade Leggings"),
        ("Light", "Leather Boots"),
        ("Light", "Shade Boots"),
        ("Heavy", "Plate Helmet"),
        ("Heavy", "Power Helmet"),
        ("Heavy", "Plate Cuirass"),
        ("Heavy", "Power Armor"),
        ("Heavy", "Plate Gauntlets"),
        ("Heavy", "Power Gauntlets"),
        ("Heavy", "Plate Greaves"),
        ("Heavy", "Power Leggings"),
        ("Heavy", "Plate Sabatons"),
        ("Heavy", "Power Boots"),
    ]
}

fn selector(raw: &str) -> Result<Selector, AdminIntakeError> {
    Selector::parse(raw).map_err(|_| AdminIntakeError::InvalidSource)
}

fn normalize_text(value: impl AsRef<str>) -> String {
    value.as_ref().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn valid_dns_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value == value.to_ascii_lowercase()
        && value.parse::<std::net::IpAddr>().is_err()
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        })
}

fn valid_path_prefix(value: &str) -> bool {
    !value.is_empty()
        && value.starts_with('/')
        && value.ends_with('/')
        && value == value.trim()
        && !value.contains("//")
        && !value
            .chars()
            .any(|character| character.is_control() || matches!(character, '?' | '#' | '%' | '\\'))
        && value.split('/').all(|segment| segment != "." && segment != "..")
}

fn valid_decimal_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 32 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn valid_optional_text(value: &str, maximum: usize) -> bool {
    value.is_empty() || valid_text(value, maximum)
}

fn sha256(bytes: Vec<u8>) -> [u8; 32] {
    let digest = Sha256::digest(bytes);
    let mut output = [0_u8; 32];
    output.copy_from_slice(&digest);
    output
}
