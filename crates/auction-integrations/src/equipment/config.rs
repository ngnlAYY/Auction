//! 装备来源适配器的启动期读取策略；冻结 host/path allowlist、响应大小、
//! 请求间隔与完整请求超时，请求路径不再放宽边界。

use std::time::Duration;

use url::Url;

pub(super) const DEFAULT_MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MINIMUM_MAX_RESPONSE_BYTES: usize = 64 * 1024;
pub(super) const DEFAULT_USER_AGENT: &str = "auction-rust-equipment-source/1";

/// 装备来源适配器初始化错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EquipmentSourceConfigurationError {
    Endpoint,
    Allowlist,
    ResponseLimit,
    RequestInterval,
    RequestTimeout,
}

impl std::fmt::Display for EquipmentSourceConfigurationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Endpoint => "装备来源 endpoint 无效",
            Self::Allowlist => "装备来源 allowlist 无效",
            Self::ResponseLimit => "装备来源响应大小限制无效",
            Self::RequestInterval => "装备来源请求间隔无效",
            Self::RequestTimeout => "装备来源请求超时无效",
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
    pub maximum_response_bytes: usize,
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
            minimum_request_interval: Duration::from_millis(250),
            request_timeout: Duration::from_secs(10),
        }
    }
}

impl EquipmentSourcePolicy {
    pub(super) fn validate(&self) -> Result<(), EquipmentSourceConfigurationError> {
        if !(MINIMUM_MAX_RESPONSE_BYTES..=DEFAULT_MAX_RESPONSE_BYTES)
            .contains(&self.maximum_response_bytes)
        {
            return Err(EquipmentSourceConfigurationError::ResponseLimit);
        }
        if self.minimum_request_interval.is_zero()
            || self.minimum_request_interval > Duration::from_secs(60)
        {
            return Err(EquipmentSourceConfigurationError::RequestInterval);
        }
        if self.request_timeout.is_zero() || self.request_timeout > Duration::from_secs(30) {
            return Err(EquipmentSourceConfigurationError::RequestTimeout);
        }
        if self.allowed_hosts.len() > 16
            || self.allowed_path_prefixes.len() > 16
            || self.allowlist_enabled
                && (self.allowed_hosts.is_empty() || self.allowed_path_prefixes.is_empty())
            || self.allowed_hosts.iter().enumerate().any(|(index, host)| {
                !auction_application::admin::valid_allowlist_host(host)
                    || self.allowed_hosts[..index].contains(host)
            })
            || self.allowed_path_prefixes.iter().enumerate().any(|(index, prefix)| {
                !auction_application::admin::valid_allowlist_path_prefix(prefix)
                    || self.allowed_path_prefixes[..index].contains(prefix)
            })
        {
            return Err(EquipmentSourceConfigurationError::Allowlist);
        }
        Ok(())
    }

    pub(super) fn allows(&self, target: &Url) -> bool {
        !self.allowlist_enabled
            || target
                .host_str()
                .is_some_and(|host| self.allowed_hosts.iter().any(|item| item == host))
                && self.allowed_path_prefixes.iter().any(|prefix| target.path().starts_with(prefix))
    }
}
