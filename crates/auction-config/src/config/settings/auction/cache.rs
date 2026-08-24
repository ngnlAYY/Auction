//! 可重建缓存策略配置。

use serde::Deserialize;

/// 可由普通配置选择的缓存后端。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheBackend {
    /// 默认模式，不加载 Redis 客户端，也不建立缓存连接。
    #[default]
    Disabled,
    /// 使用敏感配置文件中的 `redis_url`。
    Redis,
}

impl CacheBackend {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Redis => "redis",
        }
    }
}

/// 非敏感缓存策略。连接 URL 必须放在独立敏感配置文件中。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CacheConfig {
    /// 缓存后端；省略时关闭缓存。Redis 连接串只能放在敏感 TOML。
    pub backend: CacheBackend,
    /// Public 规则读模型的缓存 TTL 秒数；范围 1-3600，省略时为 30 秒。
    pub public_rules_ttl_seconds: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self { backend: CacheBackend::Disabled, public_rules_ttl_seconds: 30 }
    }
}
