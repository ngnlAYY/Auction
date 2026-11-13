//! runtime 文档的结构、大小上限与摘要版本。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    business::{
        AdminRuntimeEquipmentColorsConfig, AdminRuntimeEquipmentFilterConfig,
        AdminRuntimeWorldsConfig,
    },
    errors::AdminRuntimeConfigError,
    mail_templates::AdminRuntimeMailTemplatesConfig,
    operations::AdminRuntimeOperationsConfig,
    validation::{
        valid_fingerprint, validate_bot_players, validate_business_config,
        validate_dashboard_mailboxes, validate_equipment_source, validate_listener,
        validate_security, validate_session, validate_tracing, validate_unix_path,
    },
};
use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error,
};
use crate::ports::core::RequestHash;

/// runtime 文件和命令账本允许的最大候选字节数。
pub const MAX_ADMIN_RUNTIME_DOCUMENT_BYTES: usize = 4 << 20;

/// runtime 配置的摘要编码版本；升级编码时必须追加迁移策略，不能静默改变旧命令含义。
pub const ADMIN_RUNTIME_DIGEST_VERSION: u16 = 1;

/// Admin runtime 监听器模式。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdminRuntimeListenMode {
    #[default]
    Tcp,
    Unix,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeListenerConfig {
    /// TCP 监听地址；省略时使用 Admin 默认地址 `127.0.0.1:8091`。
    pub address: String,
    /// Unix socket 的用户组；TCP 模式下可省略。
    pub group: String,
    /// 监听模式；支持 `tcp` 与 Linux 上的 `unix`，省略时使用 `tcp`。
    pub mode: AdminRuntimeListenMode,
    /// Unix socket 运行目录；TCP 模式下可省略。
    pub runtime_directory: String,
    /// Unix socket 路径；TCP 模式下可省略。
    pub socket_path: String,
}

impl Default for AdminRuntimeListenerConfig {
    fn default() -> Self {
        Self {
            address: "127.0.0.1:8091".to_owned(),
            group: String::new(),
            mode: AdminRuntimeListenMode::Tcp,
            runtime_directory: "/run/hv-auction".to_owned(),
            socket_path: String::new(),
        }
    }
}

/// Admin 装备来源的业务 allowlist 和有界读取策略。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeEquipmentSourceConfig {
    /// 允许访问的远程主机名；默认只允许 `hentaiverse.org`。
    pub allowed_hosts: Vec<String>,
    /// 允许访问的 URL 路径前缀；默认只允许两个 world 的装备目录。
    pub allowed_path_prefixes: Vec<String>,
    /// 是否启用主机和路径双重 allowlist；省略时开启。
    pub allowlist_enabled: bool,
    /// 是否只允许可交易装备；省略时关闭。关闭值不参与序列化，以保持升级前文档指纹稳定。
    #[serde(skip_serializing_if = "is_false")]
    pub require_tradeable: bool,
    /// 单次响应最大字节数；省略时为 2 MiB。
    pub maximum_response_bytes: u64,
    /// 两次请求之间的最小间隔毫秒数；省略时沿用旧版的 250 毫秒。
    pub minimum_request_interval_millis: u64,
    /// 外部请求超时秒数；省略时为 10 秒。
    pub request_timeout_seconds: u64,
}

impl Default for AdminRuntimeEquipmentSourceConfig {
    fn default() -> Self {
        Self {
            allowed_hosts: vec!["hentaiverse.org".to_owned()],
            allowed_path_prefixes: vec!["/equip/".to_owned(), "/isekai/equip/".to_owned()],
            allowlist_enabled: true,
            require_tradeable: false,
            maximum_response_bytes: 2 << 20,
            minimum_request_interval_millis: 250,
            request_timeout_seconds: 10,
        }
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Admin runtime HTTP 安全策略；不包含登录凭据、Cookie 或签名密钥。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeSecurityConfig {
    /// 允许的精确 HTTP(S) origin；启用 allowlist 时不能为空。
    pub allowed_origins: Vec<String>,
    /// 是否启用 origin allowlist；省略时开启。
    pub allowed_origins_enabled: bool,
    /// 登录请求在时间窗口内的最大次数；省略时为 10 次。
    pub login_rate_limit: u32,
    /// 状态变更请求在时间窗口内的最大次数；省略时为 60 次。
    pub mutation_rate_limit: u32,
    /// 速率限制窗口秒数；省略时为 60 秒。
    pub rate_limit_window_seconds: u64,
    /// 是否为浏览器 Cookie 设置 Secure；省略时关闭，生产环境应开启。
    pub cookie_secure: bool,
    /// 普通 HTTP 请求的最长处理秒数；省略时为 15 秒，WebSocket 握手路径不受此限制。
    pub request_timeout_seconds: u64,
    /// 可以提供 `X-Forwarded-For` 的可信 TCP 反向代理网段；省略时不信任任何 TCP 代理。
    pub trusted_proxy_cidrs: Vec<String>,
    /// 是否信任 Unix socket 上游提供的 `X-Forwarded-For`；省略时关闭，且仅 Unix 监听模式可启用。
    pub trusted_unix_proxy: bool,
}

impl Default for AdminRuntimeSecurityConfig {
    fn default() -> Self {
        Self {
            allowed_origins: Vec::new(),
            allowed_origins_enabled: true,
            login_rate_limit: 10,
            mutation_rate_limit: 60,
            rate_limit_window_seconds: 60,
            cookie_secure: false,
            request_timeout_seconds: 15,
            trusted_proxy_cidrs: Vec::new(),
            trusted_unix_proxy: false,
        }
    }
}

/// Admin runtime 会话策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeSessionConfig {
    /// 会话空闲过期秒数；省略时沿用旧版 Admin 的 900 秒。
    pub idle_ttl_seconds: u64,
    /// 会话绝对过期秒数；省略时沿用旧版 Admin 的 28800 秒。
    pub absolute_ttl_seconds: u64,
    /// 单个公开用户允许的最大并发会话数；省略时为 10 个。
    pub max_public_sessions: u32,
}

impl Default for AdminRuntimeSessionConfig {
    fn default() -> Self {
        Self { idle_ttl_seconds: 900, absolute_ttl_seconds: 28_800, max_public_sessions: 10 }
    }
}

/// Admin runtime metrics 开关。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeMetricsConfig {
    /// 是否启用 metrics 导出；省略时关闭。
    pub enabled: bool,
}

/// Admin runtime tracing 公共参数；headers 等秘密材料永远不进入文档。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeTracingConfig {
    /// 导出批次等待秒数；省略时为 1 秒。
    pub batch_timeout_seconds: u64,
    /// 是否启用 tracing 导出；省略时关闭。
    pub enabled: bool,
    /// OTLP/HTTP 等导出端点；关闭导出时可省略或留空。
    pub endpoint: String,
    /// 单次导出超时秒数；省略时为 5 秒。
    pub export_timeout_seconds: u64,
    /// 单次导出的最大 span 数；省略时为 128。
    pub max_export_batch_size: u32,
    /// 导出队列容量；省略时为 256。
    pub queue_capacity: u32,
    /// 采样比例，单位为百万分率；省略时为 0。
    pub sample_ratio_ppm: u32,
    /// 进程关闭时等待导出完成的最大秒数；省略时为 5 秒。
    pub shutdown_timeout_seconds: u64,
}

impl Default for AdminRuntimeTracingConfig {
    fn default() -> Self {
        Self {
            batch_timeout_seconds: 1,
            enabled: false,
            endpoint: String::new(),
            export_timeout_seconds: 5,
            max_export_batch_size: 128,
            queue_capacity: 256,
            sample_ratio_ppm: 0,
            shutdown_timeout_seconds: 5,
        }
    }
}

/// Admin 连接 Worker 控制面的 Unix socket 配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeWorkerControlConfig {
    /// Worker 控制面 Unix socket 的绝对路径；省略时使用默认路径。
    pub socket_path: String,
}

impl Default for AdminRuntimeWorkerControlConfig {
    fn default() -> Self {
        Self { socket_path: "/run/hv-auction/worker/control.sock".to_owned() }
    }
}

/// Admin dashboard 需要核对的 MoogleMail world 与持久化邮箱标识。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeDashboardMailbox {
    /// 游戏 world，只允许 `main` 或 `isekai`。
    pub world_id: String,
    /// Worker 为该 world 使用的持久化邮箱标识；不包含 Cookie 或其他秘密材料。
    pub mailbox_key: String,
}

/// Admin runtime 文件允许承载的非敏感文档。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeDocument {
    /// 是否允许人工提前开放拍卖；省略时关闭。
    pub allow_manual_early_open: bool,
    /// 是否允许 Admin 提前结束竞价并把拍卖交给既有结算 Worker；省略时关闭。
    pub allow_manual_early_settlement: bool,
    /// Auction-BF 管理端可编辑的 MoogleMail world；与装备过滤策略必须同时存在或同时省略。
    /// `Option` 只用于保持升级前 runtime 文档和 durable 指纹可重放；Admin HTTP 投影会补齐
    /// 旧版默认值，新写入的文档始终包含这些业务区块。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worlds: Option<AdminRuntimeWorldsConfig>,
    /// Auction-BF 管理端可编辑的装备过滤策略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equipment_filter: Option<AdminRuntimeEquipmentFilterConfig>,
    /// 论坛装备名称标色规则；省略时不标色，保持旧 runtime 文件指纹。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equipment_colors: Option<AdminRuntimeEquipmentColorsConfig>,
    /// 历史区块，**不再生效**：通知模板已迁入数据库（见 `ports::mail_templates`），任何进程
    /// 都不再读取这里的正文，也不再校验它。区块原样保留并写回，原因有两个：升级前的程序要求
    /// 它与 `worlds`、`equipment_filter` 同时存在，保留后由新 Admin 保存过的 runtime 文件
    /// 仍能被回退到的旧程序加载；既有文档的指纹与 durable 命令摘要也不因升级而改变。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mail_templates: Option<AdminRuntimeMailTemplatesConfig>,
    /// 装备来源和远程读取边界。
    pub equipment_source: AdminRuntimeEquipmentSourceConfig,
    /// Admin HTTP 监听配置。
    pub listener: AdminRuntimeListenerConfig,
    /// Admin dashboard 用于检查 MoogleMail poller/凭据范围的非敏感邮箱清单；省略时不强制范围。
    pub dashboard_mailboxes: Vec<AdminRuntimeDashboardMailbox>,
    pub metrics: AdminRuntimeMetricsConfig,
    /// HTTP origin、速率限制和 Cookie 属性。
    pub security: AdminRuntimeSecurityConfig,
    /// Admin 会话过期和并发策略。
    pub session: AdminRuntimeSessionConfig,
    /// tracing 导出的公共参数，不包含 headers 或凭据。
    pub tracing: AdminRuntimeTracingConfig,
    /// Worker 控制面 socket 路径。
    pub worker_control: AdminRuntimeWorkerControlConfig,
    /// 收件人即 bot 的游戏显示名列表；命中时不发 COD 邮件，直接走内部履约。
    /// 空列表不参与序列化，保持升级前 runtime 文件和命令指纹不变。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bot_players: Vec<String>,
    /// 新增的分组运行参数；省略时保持旧文件和命令指纹，各组默认不接管部署配置。
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::operations::deserialize_optional"
    )]
    pub operations: Option<AdminRuntimeOperationsConfig>,
}

impl AdminRuntimeDocument {
    /// 严格校验文档；只检查可由本地文档决定的边界，不执行网络、数据库或 socket 操作。
    pub fn validate(&self) -> Result<(), AdminRuntimeConfigError> {
        validate_business_config(self)?;
        validate_listener(&self.listener)?;
        validate_dashboard_mailboxes(&self.dashboard_mailboxes)?;
        validate_equipment_source(&self.equipment_source)?;
        validate_security(&self.security)?;
        if self.security.allowed_origins_enabled && self.security.allowed_origins.is_empty() {
            if self.listener.mode != AdminRuntimeListenMode::Tcp {
                return Err(AdminRuntimeConfigError::InvalidDocument(
                    "Unix listener 启用 origin allowlist 时必须显式配置 security.allowed_origins"
                        .to_owned(),
                ));
            }
            let address = self.tcp_address()?;
            if address.ip().is_unspecified() {
                return Err(AdminRuntimeConfigError::InvalidDocument(
                    "未指定 TCP listener 无法推导浏览器 origin".to_owned(),
                ));
            }
        }
        if self.metrics.enabled
            && self.listener.mode == AdminRuntimeListenMode::Tcp
            && !self.tcp_address()?.ip().is_loopback()
        {
            return Err(AdminRuntimeConfigError::InvalidDocument(
                "启用 metrics 时 Admin listener 必须使用 loopback TCP 或 Unix".to_owned(),
            ));
        }
        if self.security.trusted_unix_proxy && self.listener.mode != AdminRuntimeListenMode::Unix {
            return Err(AdminRuntimeConfigError::InvalidDocument(
                "security.trusted_unix_proxy 只能在 Unix listener 模式启用".to_owned(),
            ));
        }
        validate_bot_players(&self.bot_players)?;
        if let Some(colors) = &self.equipment_colors {
            crate::ports::forum::validate_equipment_color_rules(&colors.to_rules())
                .map_err(AdminRuntimeConfigError::InvalidDocument)?;
        }
        validate_session(&self.session)?;
        validate_tracing(&self.tracing)?;
        validate_unix_path(&self.worker_control.socket_path, "worker_control.socket_path")?;
        if let Some(operations) = &self.operations {
            operations.validate()?;
            if operations.alerts.managed
                && operations.alerts.enabled
                && let Some(worlds) = &self.worlds
            {
                let enabled = match operations.alerts.world_id.as_str() {
                    "main" => worlds.main.enabled,
                    "isekai" => worlds.isekai.enabled,
                    _ => false,
                };
                if !enabled {
                    return Err(AdminRuntimeConfigError::InvalidDocument(
                        "operations.alerts.world_id 必须属于已启用的入站 world".to_owned(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// 为浏览器和新 runtime 文件补齐业务默认值与未接管的运行参数候选。
    /// 升级前文件在内存中继续保留 `None`，因此其既有 fingerprint 与 durable command 摘要
    /// 不会变化；只有 API 投影或用户保存的新候选会显式携带这些字段。
    pub fn materialize_business_defaults(&mut self) {
        if self.worlds.is_none() && self.equipment_filter.is_none() && self.mail_templates.is_none()
        {
            self.worlds = Some(AdminRuntimeWorldsConfig::default());
            self.equipment_filter = Some(AdminRuntimeEquipmentFilterConfig::default());
            // 模板区块已不生效；仍随另外两个区块一起补齐，保存后的文件才能被旧程序加载。
            self.mail_templates = Some(AdminRuntimeMailTemplatesConfig::default());
        }
        if let Some(equipment_filter) = &mut self.equipment_filter
            && equipment_filter.qualities.is_empty()
        {
            equipment_filter.qualities = crate::ports::mail::default_accepted_qualities();
        }
        self.equipment_colors.get_or_insert_with(Default::default);
        if self.operations.is_none() {
            self.operations = Some(AdminRuntimeOperationsConfig::default());
        }
        if let Some(operations) = &mut self.operations {
            operations.mail.get_or_insert_with(Default::default);
        }
    }

    /// 计算不依赖 TOML 排版的稳定文档指纹。
    pub fn fingerprint(&self) -> String {
        let encoded = serde_json::to_vec(self).unwrap_or_default();
        let digest = Sha256::digest(encoded);
        format!("sha256:{}", crate::ports::core::lower_hex(&digest))
    }

    /// 计算版本化的 durable 命令请求摘要。
    pub fn request_hash(
        &self,
        expected_fingerprint: &str,
    ) -> Result<RequestHash, AdminRuntimeConfigError> {
        let encoded = serde_json::to_vec(self).map_err(|error| {
            record_error(
                DiagnosticLevel::Error,
                error.diagnostic_kind(),
                "admin_runtime_config",
                "encode_request",
                DiagnosticContext::default(),
            );
            AdminRuntimeConfigError::InvalidCommand("runtime 文档编码失败".to_owned())
        })?;
        let mut hasher = Sha256::new();
        hasher.update(ADMIN_RUNTIME_DIGEST_VERSION.to_be_bytes());
        hasher.update(expected_fingerprint.as_bytes());
        hasher.update([0]);
        hasher.update(encoded);
        let mut bytes = [0_u8; 32];
        bytes.copy_from_slice(&hasher.finalize());
        Ok(RequestHash::new(bytes))
    }

    /// 返回用于启动时覆盖 Admin 普通配置的 TCP 地址；Unix 模式没有 TCP 地址。
    pub fn tcp_address(&self) -> Result<std::net::SocketAddr, AdminRuntimeConfigError> {
        if self.listener.mode != AdminRuntimeListenMode::Tcp {
            return Err(AdminRuntimeConfigError::InvalidDocument(
                "Unix runtime listener 不提供 TCP 地址".to_owned(),
            ));
        }
        self.listener.address.parse().map_err(|_| {
            AdminRuntimeConfigError::InvalidDocument(
                "listener.address 不是有效 TCP 地址".to_owned(),
            )
        })
    }
}

/// runtime 当前安全快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminRuntimeSnapshot {
    pub document: AdminRuntimeDocument,
    pub fingerprint: String,
    pub restart_required: bool,
}

impl AdminRuntimeSnapshot {
    /// 校验文件或存储适配器返回的快照，拒绝文档与指纹不一致的结果。
    pub fn validate(&self) -> Result<(), AdminRuntimeConfigError> {
        self.document.validate()?;
        if !valid_fingerprint(&self.fingerprint) || self.document.fingerprint() != self.fingerprint
        {
            return Err(AdminRuntimeConfigError::StoredResultCorrupt);
        }
        Ok(())
    }
}
