//! 应用配置输入模型、构建中间值和脱敏后的公共配置模型。

use super::super::*;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FileConfig {
    /// 兼容旧格式的可选监听地址；与 `listener` 同时提供会失败，建议使用完整 listener 区块。
    #[serde(default)]
    pub(super) listen_address: Option<String>,
    /// 可选 HTTP listener 区块；Public/Admin 可选择 TCP 或 Linux Unix socket。
    #[serde(default)]
    pub(super) listener: Option<HttpListenerConfig>,
    /// 可选 tracing 过滤器；省略时由装配根采用默认过滤器。
    #[serde(default)]
    pub(super) log_filter: Option<String>,
    /// 可选 metrics 区块；省略时关闭进程本地 Prometheus 导出。
    #[serde(default)]
    pub(super) metrics: MetricsConfig,
    /// 可选 tracing 区块；Public/Worker 使用，Admin 有 runtime 文件时由 runtime 覆盖。
    #[serde(default)]
    pub(super) tracing: TracingSettings,
    /// 可选 Public realtime 区块；省略时使用稳定 consumer 和有界 relay 默认值。
    #[serde(default)]
    pub(super) realtime: RealtimeConfig,
    /// 必填的敏感 TOML 相对或绝对路径。
    pub(super) secrets_file: PathBuf,
    /// 可选的 Admin runtime 非敏感 TOML 路径；仅 Admin 会读取。
    #[serde(default)]
    pub(super) runtime_config_file: Option<PathBuf>,
    /// 可选缓存区块；省略时使用关闭缓存的默认值。
    #[serde(default)]
    pub(super) cache: CacheConfig,
    /// 可选拍卖策略区块；省略时使用旧版共享配置的默认模板。
    #[serde(default)]
    pub(super) auction: AuctionConfig,
    /// 可选 Worker 区块；省略时 PostgreSQL 并发为 8、SQLite 并发为 1，轮询为 60 秒。
    #[serde(default)]
    pub(super) worker: WorkerConfig,
    /// 可选 HTTP 安全区块；省略时使用内置默认值。
    #[serde(default)]
    pub(super) http_security: HttpSecurityConfig,
    /// 可选会话区块；省略时使用默认过期和并发限制。
    #[serde(default)]
    pub(super) session: SessionConfig,
}

pub(super) struct BuildInput {
    pub(super) config_path: Option<PathBuf>,
    pub(super) listen_address: Option<String>,
    pub(super) listener: Option<HttpListenerConfig>,
    pub(super) log_filter: Option<String>,
    pub(super) metrics: MetricsConfig,
    pub(super) tracing: TracingSettings,
    pub(super) realtime: RealtimeConfig,
    pub(super) cache: CacheConfig,
    pub(super) auction: AuctionConfig,
    pub(super) worker: WorkerConfig,
    pub(super) http_security: HttpSecurityConfig,
    pub(super) session: SessionConfig,
    pub(super) secrets_path: Option<PathBuf>,
    pub(super) runtime_config_path: Option<PathBuf>,
}

#[derive(Clone)]
pub struct AppConfig {
    /// 当前进程角色。
    pub role: Role,
    /// 解析后的可选监听地址；Worker 通常为 None。
    pub listen_addr: Option<SocketAddr>,
    /// Public/Admin 主配置中的 listener；Worker 始终为 None。Admin runtime 存在时，实际
    /// Admin listener 由 runtime 文档覆盖，但该字段仍保留主配置的安全 fallback。
    pub listener: Option<HttpListenerConfig>,
    /// 可选日志过滤器。
    pub log_filter: Option<String>,
    /// 进程本地 Prometheus 指标导出策略。
    pub metrics: MetricsConfig,
    /// 当前角色的 OTLP/HTTP tracing 非敏感参数；Admin runtime 存在时已应用 runtime 覆盖。
    pub tracing: TracingSettings,
    /// Public realtime relay 与进程内连接 Hub 策略。
    pub realtime: RealtimeConfig,
    /// 非敏感缓存策略。
    pub cache: CacheConfig,
    /// 创建拍卖时冻结的经济、结算和恢复策略模板。
    pub auction: AuctionConfig,
    /// Worker 调度策略。
    pub worker: WorkerConfig,
    /// HTTP 安全策略。
    pub http_security: HttpSecurityConfig,
    /// 会话策略。
    pub session: SessionConfig,
    /// 主配置文件路径；程序化默认配置时为 None。
    pub config_path: Option<PathBuf>,
    /// 敏感配置文件路径；主配置加载时由必填 secrets_file 解析得到。
    pub secrets_path: Option<PathBuf>,
    /// Admin runtime 文件路径；非 Admin 或省略 runtime_config_file 时为 None。
    pub runtime_config_path: Option<PathBuf>,
    /// 已校验的 Admin runtime 非敏感文档；Worker/Public 始终为 None。
    pub admin_runtime: Option<AdminRuntimeDocument>,
    pub(super) secrets: AppSecrets,
}

impl fmt::Debug for AppConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AppConfig")
            .field("role", &self.role)
            .field("listen_addr", &self.listen_addr)
            .field("listener", &self.listener)
            .field("database_configured", &self.database_url().is_some())
            .field("admin_credentials_configured", &self.admin_credentials_configured())
            .field("cache", &self.cache)
            .field("auction", &self.auction)
            .field("worker", &self.worker)
            .field("http_security", &self.http_security)
            .field("session", &self.session)
            .field("log_filter", &self.log_filter)
            .field("metrics", &self.metrics)
            .field("tracing", &self.tracing)
            .field("realtime", &self.realtime)
            .field("config_path", &self.config_path)
            .field("secrets_path", &self.secrets_path)
            .field("runtime_config_path", &self.runtime_config_path)
            .field("admin_runtime_configured", &self.admin_runtime.is_some())
            .finish()
    }
}
