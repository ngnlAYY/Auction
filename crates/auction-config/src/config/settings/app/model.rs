//! 应用配置输入模型、构建中间值和脱敏后的公共配置模型。

use super::super::*;
use auction_application::EquipmentColorRule;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FileConfig {
    /// 必填的公共非敏感 TOML 路径；共享日志、指标、缓存和拍卖策略只从该文件加载。
    pub(super) common_file: PathBuf,
    /// 兼容旧格式的可选监听地址；与 `listener` 同时提供会失败，建议使用完整 listener 区块。
    #[serde(default)]
    pub(super) listen_address: Option<String>,
    /// 可选 HTTP listener 区块；Public/Admin 可选择 TCP 或 Linux Unix socket。
    #[serde(default)]
    pub(super) listener: Option<HttpListenerConfig>,
    /// 可选 tracing 区块；Public/Worker 使用，Admin 有 runtime 文件时由 runtime 覆盖。
    #[serde(default)]
    pub(super) tracing: TracingSettings,
    /// 可选 Public realtime 区块；省略时使用稳定 consumer 和有界 relay 默认值。
    #[serde(default)]
    pub(super) realtime: RealtimeConfig,
    /// 必填的敏感 TOML 相对或绝对路径。
    pub(super) secrets_file: PathBuf,
    /// 可选的 Admin 管理 runtime 非敏感 TOML 路径；三角色共享同一文件，Public 只投影
    /// 装备分组规则，Worker 投影邮件业务、论坛规则及 bot 内部履约名单。
    #[serde(default)]
    pub(super) runtime_config_file: Option<PathBuf>,
    /// 可选 Worker 区块；省略时并发为 8，轮询为 60 秒。
    #[serde(default)]
    pub(super) worker: WorkerConfig,
    /// 可选 HTTP 安全区块；省略时按进程角色使用内置默认值。
    #[serde(default)]
    pub(super) http_security: Option<HttpSecurityConfig>,
    /// 可选会话区块；省略时按 Public/Admin 使用各自的旧版默认值。
    #[serde(default)]
    pub(super) session: Option<SessionConfig>,
}

/// 三个进程共同引用的非敏感配置；闭集模型同时阻止角色专属字段混入公共文件。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct CommonFileConfig {
    /// 可选 tracing 过滤器；省略时由装配根采用默认过滤器。
    pub(super) log_filter: Option<String>,
    /// 进程本地 Prometheus 指标公共默认值；Admin runtime 仍可在重启时覆盖。
    pub(super) metrics: MetricsConfig,
    pub(super) cache: CacheConfig,
    /// 创建拍卖时冻结的计划、经济、结算和恢复策略。
    pub(super) auction: AuctionConfig,
}

pub(super) struct BuildInput {
    pub(super) config_path: Option<PathBuf>,
    pub(super) common_config_path: Option<PathBuf>,
    pub(super) listen_address: Option<String>,
    pub(super) listener: Option<HttpListenerConfig>,
    pub(super) log_filter: Option<String>,
    pub(super) metrics: MetricsConfig,
    pub(super) tracing: TracingSettings,
    pub(super) realtime: RealtimeConfig,
    pub(super) cache: CacheConfig,
    pub(super) auction: AuctionConfig,
    pub(super) worker: WorkerConfig,
    pub(super) http_security: Option<HttpSecurityConfig>,
    pub(super) session: Option<SessionConfig>,
    pub(super) secrets_path: Option<PathBuf>,
    pub(super) runtime_config_path: Option<PathBuf>,
}

#[derive(Clone)]
pub struct AppConfig {
    pub role: Role,
    /// 解析后的可选监听地址；Worker 通常为 None。
    pub listen_addr: Option<SocketAddr>,
    /// Public/Admin 当前生效的 listener；Worker 始终为 None。Admin runtime 存在时，字段已
    /// 按 runtime 文档覆盖，主配置只在 runtime 缺失时作为 fallback。
    pub listener: Option<HttpListenerConfig>,
    pub log_filter: Option<String>,
    pub metrics: MetricsConfig,
    /// 当前角色的 OTLP/HTTP tracing 非敏感参数；Admin runtime 存在时已应用 runtime 覆盖。
    pub tracing: TracingSettings,
    /// Public realtime relay 与进程内连接 Hub 策略。
    pub realtime: RealtimeConfig,
    /// 非敏感缓存策略。
    pub cache: CacheConfig,
    /// 创建拍卖时冻结的经济、结算和恢复策略模板。
    pub auction: AuctionConfig,
    pub worker: WorkerConfig,
    pub http_security: HttpSecurityConfig,
    pub session: SessionConfig,
    /// 主配置文件路径；程序化默认配置时为 None。
    pub config_path: Option<PathBuf>,
    /// 公共非敏感配置文件路径；文件配置加载时由必填 common_file 解析得到。
    pub common_config_path: Option<PathBuf>,
    /// 敏感配置文件路径；主配置加载时由必填 secrets_file 解析得到。
    pub secrets_path: Option<PathBuf>,
    /// Admin 管理 runtime 文件路径；三角色可配置，省略时为 None。
    pub runtime_config_path: Option<PathBuf>,
    /// 已校验并应用到 Admin 进程的 runtime 文档；Public/Worker 只保留窄投影，因此为 None。
    pub admin_runtime: Option<AdminRuntimeDocument>,
    /// 三角色同源的装备规则启动快照；Public 判定稀有/融合，Worker 用于论坛名称标色。
    pub equipment_colors: Vec<EquipmentColorRule>,
    /// 仅供 Worker 内部履约使用的 runtime bot 显示名投影；其他角色或未配置时为空。
    pub worker_bot_players: Vec<String>,
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
            .field("common_config_path", &self.common_config_path)
            .field("secrets_path", &self.secrets_path)
            .field("runtime_config_path", &self.runtime_config_path)
            .field("admin_runtime_configured", &self.admin_runtime.is_some())
            .finish()
    }
}
