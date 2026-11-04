//! HTTP 安全和会话配置模型。

use auction_application::AdminRuntimeSecurityConfig;

use super::*;

/// 浏览器写请求和公开会话使用的非敏感 HTTP 安全策略。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HttpSecurityConfig {
    /// 允许的精确 HTTP(S) origin；启用 allowlist 时必须填写。
    pub allowed_origins: Vec<String>,
    /// 是否启用 origin allowlist；Public/Admin 省略整个区块时开启。
    pub allowed_origins_enabled: bool,
    /// 登录请求在一个窗口内的最大次数；省略时为 10 次。
    pub login_rate_limit: u32,
    /// 状态变更请求在一个窗口内的最大次数；Public realtime 连接尝试使用相同数值但独立计数，
    /// 省略时为 60 次。
    pub mutation_rate_limit: u32,
    /// 速率限制窗口秒数；省略时为 60 秒。
    pub rate_limit_window_seconds: u64,
    /// 是否为浏览器 Cookie 设置 Secure；省略时关闭，生产环境应开启。
    pub cookie_secure: bool,
    /// HTTP 请求和 WebSocket 握手的最长处理秒数；省略时为 15 秒，不限制升级后的连接生命周期。
    pub request_timeout_seconds: u64,
    /// 可以提供 `X-Forwarded-For` 的可信反向代理网段；省略时不信任任何 TCP 代理。
    pub trusted_proxy_cidrs: Vec<String>,
    /// 是否信任 Unix socket 上游提供的 `X-Forwarded-For`；省略时关闭，仅允许 Public/Admin
    /// Unix 监听器启用。
    pub trusted_unix_proxy: bool,
}

/// 部署 TOML 省略字段时与 runtime 文档使用同一组默认值，由 application 的 runtime 模型唯一持有。
impl Default for HttpSecurityConfig {
    fn default() -> Self {
        Self::from(&AdminRuntimeSecurityConfig::default())
    }
}

/// 将 Admin runtime 的 HTTP 部分投影为公共进程配置；不携带运行时密文。
impl From<&AdminRuntimeSecurityConfig> for HttpSecurityConfig {
    fn from(security: &AdminRuntimeSecurityConfig) -> Self {
        Self {
            allowed_origins: security.allowed_origins.clone(),
            allowed_origins_enabled: security.allowed_origins_enabled,
            login_rate_limit: security.login_rate_limit,
            mutation_rate_limit: security.mutation_rate_limit,
            rate_limit_window_seconds: security.rate_limit_window_seconds,
            cookie_secure: security.cookie_secure,
            request_timeout_seconds: security.request_timeout_seconds,
            trusted_proxy_cidrs: security.trusted_proxy_cidrs.clone(),
            trusted_unix_proxy: security.trusted_unix_proxy,
        }
    }
}

impl HttpSecurityConfig {
    /// 返回角色对应的旧版安全默认值；Worker 没有浏览器 HTTP 表面，因此不启用 origin 校验。
    pub(crate) fn for_role(role: Role) -> Self {
        let mut config = Self::default();
        if role == Role::Worker {
            config.allowed_origins_enabled = false;
        }
        config
    }
}

/// Admin 和 Worker 保留的会话时间配置；Public 不使用该模型或其中的部署时限。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionConfig {
    /// 会话空闲过期秒数；省略时为 1800 秒。
    pub idle_ttl_seconds: u64,
    /// 会话绝对过期秒数；省略时为 604800 秒。
    pub absolute_ttl_seconds: u64,
    /// 单个公开用户允许的最大并发会话数；省略时为 10 个。
    pub max_public_sessions: u32,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            idle_ttl_seconds: 30 * 60,
            absolute_ttl_seconds: 7 * 24 * 60 * 60,
            max_public_sessions: 10,
        }
    }
}

/// Public 部署只拥有并发上限；登录时长由账号偏好拥有，未设置时默认永久。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicSessionSettings {
    /// 单个公开用户允许的最大并发会话数；省略时为 10 个。
    pub max_public_sessions: u32,
}

impl Default for PublicSessionSettings {
    fn default() -> Self {
        Self { max_public_sessions: 10 }
    }
}

/// 按角色隔离有效会话配置，防止 Public 装配继续读取 Admin 的时间策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleSessionConfig {
    Public(PublicSessionSettings),
    /// 保留 Admin 的解析、runtime 投影以及 Worker 原有的非生效会话配置。
    Timed(SessionConfig),
}

impl RoleSessionConfig {
    /// Public 不设置服务端到期；Admin 省略整个区块时仍为 15 分钟空闲和 8 小时绝对时限。
    pub(crate) fn for_role(role: Role) -> Self {
        match role {
            Role::Public => Self::Public(PublicSessionSettings::default()),
            Role::Admin => Self::Timed(SessionConfig {
                idle_ttl_seconds: 15 * 60,
                absolute_ttl_seconds: 8 * 60 * 60,
                max_public_sessions: 10,
            }),
            Role::Worker => Self::Timed(SessionConfig::default()),
        }
    }
}
