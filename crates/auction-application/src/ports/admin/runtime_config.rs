//! Admin runtime 配置的文档、指纹和持久化命令端口。
//!
//! runtime 文档只承载非敏感字段。文件适配器负责 TOML 编解码和原子替换，数据库适配器负责
//! durable intent、审计和命令状态；本模块只定义两者之间的窄边界，不读取进程配置或执行 SQL。

use std::path::Path;

use chrono::{DateTime, Utc};
use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use auction_domain::CommandId;

use crate::ApplicationError;

use super::{ApplicationFuture, RequestHash};

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

/// Admin runtime 监听器配置。
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
    /// 单次响应最大字节数；省略时为 2 MiB。
    pub maximum_response_bytes: u64,
    /// 两次请求之间的最小间隔秒数；省略时为 1 秒。
    pub minimum_request_interval_seconds: u64,
    /// 外部请求超时秒数；省略时为 10 秒。
    pub request_timeout_seconds: u64,
}

impl Default for AdminRuntimeEquipmentSourceConfig {
    fn default() -> Self {
        Self {
            allowed_hosts: vec!["hentaiverse.org".to_owned()],
            allowed_path_prefixes: vec!["/equip/".to_owned(), "/isekai/equip/".to_owned()],
            allowlist_enabled: true,
            maximum_response_bytes: 2 << 20,
            minimum_request_interval_seconds: 1,
            request_timeout_seconds: 10,
        }
    }
}

/// Admin runtime HTTP 安全策略；不包含登录凭据、Cookie 或签名密钥。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeSecurityConfig {
    /// 允许的精确 HTTP(S) origin；启用 allowlist 时不能为空。
    pub allowed_origins: Vec<String>,
    /// 是否启用 origin allowlist；省略时关闭。
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
            allowed_origins_enabled: false,
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
    /// 会话空闲过期秒数；省略时为 1800 秒。
    pub idle_ttl_seconds: u64,
    /// 会话绝对过期秒数；省略时为 604800 秒。
    pub absolute_ttl_seconds: u64,
    /// 单个公开用户允许的最大并发会话数；省略时为 5 个。
    pub max_public_sessions: u32,
}

impl Default for AdminRuntimeSessionConfig {
    fn default() -> Self {
        Self { idle_ttl_seconds: 1800, absolute_ttl_seconds: 604800, max_public_sessions: 5 }
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
    /// 装备来源和远程读取边界。
    pub equipment_source: AdminRuntimeEquipmentSourceConfig,
    /// Admin HTTP 监听配置。
    pub listener: AdminRuntimeListenerConfig,
    /// Admin dashboard 用于检查 MoogleMail poller/凭据范围的非敏感邮箱清单；省略时不强制范围。
    pub dashboard_mailboxes: Vec<AdminRuntimeDashboardMailbox>,
    /// metrics 公共开关。
    pub metrics: AdminRuntimeMetricsConfig,
    /// HTTP origin、速率限制和 Cookie 属性。
    pub security: AdminRuntimeSecurityConfig,
    /// Admin 会话过期和并发策略。
    pub session: AdminRuntimeSessionConfig,
    /// tracing 导出的公共参数，不包含 headers 或凭据。
    pub tracing: AdminRuntimeTracingConfig,
    /// Worker 控制面 socket 路径。
    pub worker_control: AdminRuntimeWorkerControlConfig,
}

impl AdminRuntimeDocument {
    /// 严格校验文档；只检查可由本地文档决定的边界，不执行网络、数据库或 socket 操作。
    pub fn validate(&self) -> Result<(), AdminRuntimeConfigError> {
        validate_listener(&self.listener)?;
        validate_dashboard_mailboxes(&self.dashboard_mailboxes)?;
        validate_equipment_source(&self.equipment_source)?;
        validate_security(&self.security)?;
        if self.security.trusted_unix_proxy && self.listener.mode != AdminRuntimeListenMode::Unix {
            return Err(AdminRuntimeConfigError::InvalidDocument(
                "security.trusted_unix_proxy 只能在 Unix listener 模式启用".to_owned(),
            ));
        }
        validate_session(&self.session)?;
        validate_tracing(&self.tracing)?;
        validate_unix_path(&self.worker_control.socket_path, "worker_control.socket_path")?;
        Ok(())
    }

    /// 计算不依赖 TOML 排版的稳定文档指纹。
    pub fn fingerprint(&self) -> String {
        let encoded = serde_json::to_vec(self).unwrap_or_default();
        let digest = Sha256::digest(encoded);
        format!("sha256:{digest:x}")
    }

    /// 计算版本化的 durable 命令请求摘要。
    pub fn request_hash(
        &self,
        expected_fingerprint: &str,
    ) -> Result<RequestHash, AdminRuntimeConfigError> {
        let encoded = serde_json::to_vec(self).map_err(|_| {
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

/// Admin runtime 更新命令；HTTP、Worker 和 CLI 入口都复用这组 fence/摘要语义。
#[derive(Debug, Clone)]
pub struct AdminRuntimeUpdateCommand {
    pub command_id: CommandId,
    pub expected_fingerprint: String,
    pub document: AdminRuntimeDocument,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub request_hash: RequestHash,
}

impl AdminRuntimeUpdateCommand {
    /// 校验命令的输入边界和版本化请求摘要。
    pub fn validate(&self) -> Result<(), AdminRuntimeConfigError> {
        if self.command_id.is_nil()
            || !valid_fingerprint(&self.expected_fingerprint)
            || !valid_text(&self.actor_id, 256)
            || self.request_id.as_deref().is_some_and(|value| !valid_text(value, 256))
            || self.document.validate().is_err()
        {
            return Err(AdminRuntimeConfigError::InvalidCommand(
                "runtime 更新命令字段无效".to_owned(),
            ));
        }
        if self.document.request_hash(&self.expected_fingerprint)? != self.request_hash {
            return Err(AdminRuntimeConfigError::InvalidCommand(
                "runtime 更新命令摘要不匹配".to_owned(),
            ));
        }
        Ok(())
    }
}

/// durable runtime 命令的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdminRuntimeCommandState {
    Pending,
    Applied,
    Unknown,
}

/// runtime 文件完成替换后、数据库命令尚未最终提交时的安全替换事实。
#[derive(Debug, Clone)]
pub struct AdminRuntimeReplacement {
    pub old_document: AdminRuntimeDocument,
    pub old_fingerprint: String,
    pub new_document: AdminRuntimeDocument,
    pub new_fingerprint: String,
}

/// runtime 命令账本安全投影。
#[derive(Debug, Clone)]
pub struct AdminRuntimeStoredCommand {
    pub command_id: CommandId,
    pub audit_id: Uuid,
    pub digest_version: u16,
    pub request_hash: RequestHash,
    pub expected_fingerprint: String,
    pub old_fingerprint: String,
    pub new_fingerprint: String,
    pub candidate_document: AdminRuntimeDocument,
    pub result_document: Option<AdminRuntimeDocument>,
    pub restart_required: bool,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub occurred_at: DateTime<Utc>,
    pub state: AdminRuntimeCommandState,
    pub unknown_category: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub applied_at: Option<DateTime<Utc>>,
}

impl AdminRuntimeStoredCommand {
    /// 校验账本行能否重建可信的候选和结果。
    pub fn validate(&self) -> Result<(), AdminRuntimeConfigError> {
        if self.command_id.is_nil()
            || self.audit_id.is_nil()
            || self.digest_version != ADMIN_RUNTIME_DIGEST_VERSION
            || self.request_hash.as_bytes() == [0; 32]
            || !valid_fingerprint(&self.expected_fingerprint)
            || self.expected_fingerprint != self.old_fingerprint
            || !valid_fingerprint(&self.new_fingerprint)
            || !valid_text(&self.actor_id, 256)
            || self.request_id.as_deref().is_some_and(|value| !valid_text(value, 256))
            || self.occurred_at.timestamp_micros() == 0
            || self.created_at.timestamp_micros() == 0
            || self.updated_at < self.created_at
            || !self.restart_required
            || self.candidate_document.validate().is_err()
            || self.candidate_document.fingerprint() != self.new_fingerprint
            || self.candidate_document.request_hash(&self.expected_fingerprint).ok()
                != Some(self.request_hash)
        {
            return Err(AdminRuntimeConfigError::StoredResultCorrupt);
        }
        match self.state {
            AdminRuntimeCommandState::Pending => {
                if self.result_document.is_some()
                    || self.applied_at.is_some()
                    || self.unknown_category.is_some()
                {
                    return Err(AdminRuntimeConfigError::StoredResultCorrupt);
                }
            }
            AdminRuntimeCommandState::Unknown => {
                if self.result_document.is_some()
                    || self.applied_at.is_some()
                    || !self.unknown_category.as_deref().is_some_and(valid_unknown_category)
                {
                    return Err(AdminRuntimeConfigError::StoredResultCorrupt);
                }
            }
            AdminRuntimeCommandState::Applied => {
                let Some(result) = &self.result_document else {
                    return Err(AdminRuntimeConfigError::StoredResultCorrupt);
                };
                if self.applied_at.is_none()
                    || self.unknown_category.is_some()
                    || result.validate().is_err()
                    || result.fingerprint() != self.new_fingerprint
                    || result != &self.candidate_document
                {
                    return Err(AdminRuntimeConfigError::StoredResultCorrupt);
                }
            }
        }
        Ok(())
    }

    /// 判断已有命令是否与新的请求完全相同。
    pub fn matches_request(&self, request_hash: RequestHash) -> bool {
        self.digest_version == ADMIN_RUNTIME_DIGEST_VERSION && self.request_hash == request_hash
    }
}

/// 数据库审计写入所需的安全事实。
#[derive(Debug, Clone)]
pub struct AdminRuntimeAuditRecord {
    pub id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub command_id: CommandId,
    pub old_fingerprint: String,
    pub new_fingerprint: String,
}

/// runtime 文件适配器端口。
pub trait AdminRuntimeFileStore: Send + Sync {
    fn current(&self) -> ApplicationFuture<'_, AdminRuntimeSnapshot>;

    fn replace(
        &self,
        expected_fingerprint: String,
        document: AdminRuntimeDocument,
    ) -> ApplicationFuture<'_, AdminRuntimeReplacement>;

    fn restore(&self, replacement: AdminRuntimeReplacement) -> ApplicationFuture<'_, ()>;
}

/// runtime 命令账本适配器端口。
pub trait AdminRuntimeCommandStore: Send + Sync {
    fn find(
        &self,
        command_id: CommandId,
    ) -> ApplicationFuture<'_, Option<AdminRuntimeStoredCommand>>;

    fn list_unresolved(&self) -> ApplicationFuture<'_, Vec<AdminRuntimeStoredCommand>>;

    fn insert_pending(
        &self,
        command: AdminRuntimeStoredCommand,
    ) -> ApplicationFuture<'_, AdminRuntimeStoredCommand>;

    fn apply(
        &self,
        command_id: CommandId,
        request_hash: RequestHash,
        audit: AdminRuntimeAuditRecord,
        applied_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, AdminRuntimeStoredCommand>;

    fn mark_unknown(
        &self,
        command_id: CommandId,
        request_hash: RequestHash,
        category: String,
        updated_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, AdminRuntimeStoredCommand>;

    fn reopen(
        &self,
        command_id: CommandId,
        request_hash: RequestHash,
        updated_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, AdminRuntimeStoredCommand>;
}

/// Admin runtime 配置应用端口。
pub trait AdminRuntimeConfigPort: Send + Sync {
    fn current(&self) -> ApplicationFuture<'_, AdminRuntimeSnapshot>;

    fn validate(
        &self,
        document: AdminRuntimeDocument,
    ) -> Result<AdminRuntimeSnapshot, ApplicationError>;

    fn update(
        &self,
        command: AdminRuntimeUpdateCommand,
    ) -> ApplicationFuture<'_, AdminRuntimeSnapshot>;
}

/// runtime 文档/命令错误。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AdminRuntimeConfigError {
    #[error("runtime 配置文档无效：{0}")]
    InvalidDocument(String),
    #[error("runtime 配置命令无效：{0}")]
    InvalidCommand(String),
    #[error("runtime 配置 fingerprint 已过期")]
    StaleFingerprint,
    #[error("runtime 配置 command 冲突")]
    CommandConflict,
    #[error("runtime 配置更新结果未知")]
    OutcomeUnknown,
    #[error("runtime 配置需要人工核对")]
    ReconciliationRequired,
    #[error("runtime 配置持久化命令损坏")]
    StoredResultCorrupt,
}

/// runtime 文件适配器错误。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AdminRuntimeFileError {
    #[error("runtime 配置文件不可用：{0}")]
    Unavailable(String),
    #[error("runtime 配置文件冲突")]
    Conflict,
    #[error("runtime 配置文件原子替换结果未知")]
    OutcomeUnknown,
}

/// runtime 命令账本适配器错误。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AdminRuntimeCommandError {
    #[error("runtime 配置命令不存在")]
    NotFound,
    #[error("runtime 配置命令发生唯一冲突")]
    Conflict,
    #[error("runtime 配置命令存储失败：{0}")]
    Failed(String),
}

fn validate_listener(config: &AdminRuntimeListenerConfig) -> Result<(), AdminRuntimeConfigError> {
    if (!config.address.is_empty() && !valid_text(&config.address, 256))
        || (!config.group.is_empty() && !valid_text(&config.group, 128))
        || (!config.runtime_directory.is_empty() && !valid_text(&config.runtime_directory, 512))
        || (!config.socket_path.is_empty() && !valid_text(&config.socket_path, 512))
    {
        return Err(AdminRuntimeConfigError::InvalidDocument("listener 字段无效".to_owned()));
    }
    match config.mode {
        AdminRuntimeListenMode::Tcp => {
            if !valid_text(&config.address, 256)
                || !config.group.is_empty()
                || !config.socket_path.is_empty()
                || config.address.parse::<std::net::SocketAddr>().is_err()
            {
                return Err(AdminRuntimeConfigError::InvalidDocument(
                    "TCP listener 字段无效".to_owned(),
                ));
            }
        }
        AdminRuntimeListenMode::Unix => {
            if !config.address.is_empty()
                || !valid_group_name(&config.group)
                || validate_unix_path(&config.runtime_directory, "listener.runtime_directory")
                    .is_err()
                || validate_unix_path(&config.socket_path, "listener.socket_path").is_err()
                || Path::new(&config.socket_path).parent()
                    != Some(Path::new(&config.runtime_directory))
            {
                return Err(AdminRuntimeConfigError::InvalidDocument(
                    "Unix listener 必须提供规范化的 runtime_directory、socket_path 和 group"
                        .to_owned(),
                ));
            }
        }
    }
    Ok(())
}

fn validate_dashboard_mailboxes(
    mailboxes: &[AdminRuntimeDashboardMailbox],
) -> Result<(), AdminRuntimeConfigError> {
    if mailboxes.len() > 2 {
        return Err(AdminRuntimeConfigError::InvalidDocument(
            "dashboard_mailboxes 最多只能配置两个 world".to_owned(),
        ));
    }
    let mut world_ids = Vec::with_capacity(mailboxes.len());
    let mut mailbox_keys = Vec::with_capacity(mailboxes.len());
    for mailbox in mailboxes {
        if !matches!(mailbox.world_id.as_str(), "main" | "isekai")
            || !valid_text(&mailbox.mailbox_key, 256)
            || world_ids.iter().any(|world| world == &mailbox.world_id)
            || mailbox_keys.iter().any(|key| key == &mailbox.mailbox_key)
        {
            return Err(AdminRuntimeConfigError::InvalidDocument(
                "dashboard_mailboxes 的 world_id/mailbox_key 无效或重复".to_owned(),
            ));
        }
        world_ids.push(mailbox.world_id.clone());
        mailbox_keys.push(mailbox.mailbox_key.clone());
    }
    Ok(())
}

fn validate_equipment_source(
    config: &AdminRuntimeEquipmentSourceConfig,
) -> Result<(), AdminRuntimeConfigError> {
    if config.allowed_hosts.len() > 16
        || config.allowed_path_prefixes.len() > 16
        || !(64 << 10..=8 << 20).contains(&config.maximum_response_bytes)
        || !(1..=60).contains(&config.minimum_request_interval_seconds)
        || !(1..=30).contains(&config.request_timeout_seconds)
        || config.allowlist_enabled && config.allowed_hosts.is_empty()
        || config.allowlist_enabled && config.allowed_path_prefixes.is_empty()
    {
        return Err(AdminRuntimeConfigError::InvalidDocument(
            "equipment_source 边界无效".to_owned(),
        ));
    }
    if config
        .allowed_hosts
        .iter()
        .enumerate()
        .any(|(index, host)| !valid_dns_name(host) || config.allowed_hosts[..index].contains(host))
        || config.allowed_path_prefixes.iter().enumerate().any(|(index, prefix)| {
            !valid_path_prefix(prefix) || config.allowed_path_prefixes[..index].contains(prefix)
        })
    {
        return Err(AdminRuntimeConfigError::InvalidDocument(
            "equipment_source allowlist 格式无效".to_owned(),
        ));
    }
    Ok(())
}

fn validate_security(config: &AdminRuntimeSecurityConfig) -> Result<(), AdminRuntimeConfigError> {
    if config.login_rate_limit == 0
        || config.login_rate_limit > 10_000
        || config.mutation_rate_limit == 0
        || config.mutation_rate_limit > 10_000
        || !(1..=3600).contains(&config.rate_limit_window_seconds)
        || !(1..=120).contains(&config.request_timeout_seconds)
        || config.allowed_origins.len() > 16
        || config.trusted_proxy_cidrs.len() > 64
    {
        return Err(AdminRuntimeConfigError::InvalidDocument("security 边界无效".to_owned()));
    }
    if config.allowed_origins_enabled
        && (config.allowed_origins.is_empty()
            || config.allowed_origins.iter().any(|origin| !valid_origin(origin)))
    {
        return Err(AdminRuntimeConfigError::InvalidDocument(
            "security.allowed_origins 无效".to_owned(),
        ));
    }
    for (index, value) in config.trusted_proxy_cidrs.iter().enumerate() {
        let Ok(network) = value.parse::<IpNet>() else {
            return Err(AdminRuntimeConfigError::InvalidDocument(
                "security.trusted_proxy_cidrs 无效".to_owned(),
            ));
        };
        if network.trunc().to_string() != *value
            || config.trusted_proxy_cidrs[..index].iter().any(|existing| existing == value)
        {
            return Err(AdminRuntimeConfigError::InvalidDocument(
                "security.trusted_proxy_cidrs 必须规范且不能重复".to_owned(),
            ));
        }
    }
    Ok(())
}

fn validate_session(config: &AdminRuntimeSessionConfig) -> Result<(), AdminRuntimeConfigError> {
    if !(60..=86_400).contains(&config.idle_ttl_seconds)
        || config.absolute_ttl_seconds < config.idle_ttl_seconds
        || config.absolute_ttl_seconds > 2_592_000
        || config.max_public_sessions == 0
        || config.max_public_sessions > 100
    {
        return Err(AdminRuntimeConfigError::InvalidDocument("session 边界无效".to_owned()));
    }
    Ok(())
}

fn validate_tracing(config: &AdminRuntimeTracingConfig) -> Result<(), AdminRuntimeConfigError> {
    if !(1..=30).contains(&config.batch_timeout_seconds)
        || !(1..=30).contains(&config.export_timeout_seconds)
        || !(1..=30).contains(&config.shutdown_timeout_seconds)
        || !(1..=512).contains(&config.max_export_batch_size)
        || !(1..=4096).contains(&config.queue_capacity)
        || config.max_export_batch_size > config.queue_capacity
        || config.sample_ratio_ppm > 1_000_000
        || config.enabled && !valid_http_url(&config.endpoint)
        || !config.enabled && !config.endpoint.is_empty() && !valid_http_url(&config.endpoint)
    {
        return Err(AdminRuntimeConfigError::InvalidDocument("tracing 边界无效".to_owned()));
    }
    Ok(())
}

fn validate_unix_path(value: &str, field: &str) -> Result<(), AdminRuntimeConfigError> {
    if value.is_empty()
        || value.len() > 512
        || !value.starts_with('/')
        || value == "/"
        || value != value.trim()
        || value.chars().any(char::is_control)
        || value.contains("//")
        || value.split('/').any(|segment| segment == "." || segment == "..")
    {
        return Err(AdminRuntimeConfigError::InvalidDocument(format!("{field} 无效")));
    }
    Ok(())
}

fn valid_group_name(value: &str) -> bool {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    value.len() <= 64
        && first.is_ascii_alphanumeric()
        && characters
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
}

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn valid_fingerprint(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..].chars().all(|character| character.is_ascii_hexdigit())
        && value == value.to_ascii_lowercase()
}

fn valid_unknown_category(value: &str) -> bool {
    matches!(
        value,
        "current_unavailable"
            | "current_corrupt"
            | "replace_unknown"
            | "result_corrupt"
            | "file_conflict"
    )
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

fn valid_origin(value: &str) -> bool {
    let Some((scheme, host)) = value.split_once("://") else {
        return false;
    };
    if !matches!(scheme, "http" | "https")
        || host.is_empty()
        || host.contains('/')
        || host.chars().any(|character| matches!(character, '?' | '#' | '@'))
    {
        return false;
    }
    let Some((hostname, port)) = host.rsplit_once(':') else {
        return valid_dns_name(host);
    };
    valid_dns_name(hostname) && !port.is_empty() && port.parse::<u16>().is_ok()
}

fn valid_http_url(value: &str) -> bool {
    (value.starts_with("http://") || value.starts_with("https://"))
        && !value
            .chars()
            .any(|character| matches!(character, ' ' | '\n' | '\r' | '\0' | '?' | '#' | '@'))
        && value.split_once("://").is_some_and(|(_, host)| !host.is_empty())
}
