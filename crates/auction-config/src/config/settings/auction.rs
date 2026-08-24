//! 拍卖、缓存、指标、tracing 和 realtime 配置模型。

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Public,
    Admin,
    Worker,
}

impl Role {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Admin => "admin",
            Self::Worker => "worker",
        }
    }

    pub(super) const fn default_listen(self) -> Option<&'static str> {
        match self {
            Self::Public => Some("127.0.0.1:8090"),
            Self::Admin => Some("127.0.0.1:8091"),
            Self::Worker => None,
        }
    }
}

/// HTTP 监听器允许使用的部署模式。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HttpListenMode {
    /// 监听 TCP host:port。
    #[default]
    Tcp,
    /// 监听 Linux Unix socket。
    Unix,
}

impl HttpListenMode {
    /// 返回稳定的配置/日志名称。
    pub const fn name(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Unix => "unix",
        }
    }
}

/// Public/Admin HTTP 监听器的非敏感配置。
///
/// TCP 模式只使用 `address`；Unix 模式要求 `runtime_directory`、`socket_path` 和 `group`
/// 同时提供。socket 路径必须直接位于 runtime 目录下，实际绑定时还会再次检查目录、锁、
/// owner/group、权限和 socket 身份。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HttpListenerConfig {
    /// TCP 监听地址；省略时 Public/Admin 分别使用各自的 loopback 默认端口。
    pub address: String,
    /// Unix socket 所属用户组；TCP 模式下必须为空。
    pub group: String,
    /// 监听模式；可填 `tcp` 或 Linux 上的 `unix`，省略时为 `tcp`。
    pub mode: HttpListenMode,
    /// Unix socket 运行目录；TCP 模式下必须为空。
    pub runtime_directory: String,
    /// Unix socket 路径；TCP 模式下必须为空。
    pub socket_path: String,
}

impl HttpListenerConfig {
    /// 创建一个 TCP 监听配置。
    pub fn tcp(address: impl Into<String>) -> Self {
        Self { address: address.into(), ..Self::default() }
    }
}

impl Default for HttpListenerConfig {
    fn default() -> Self {
        Self {
            address: String::new(),
            group: String::new(),
            mode: HttpListenMode::Tcp,
            runtime_directory: String::new(),
            socket_path: String::new(),
        }
    }
}

impl FromStr for Role {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "public" => Ok(Self::Public),
            "admin" => Ok(Self::Admin),
            "worker" => Ok(Self::Worker),
            _ => Err(ConfigError::UnknownRole(value.to_owned())),
        }
    }
}

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

/// 拍卖创建时冻结的非敏感策略配置。
///
/// 该区块对应旧版共享配置中的 `auction.economics` 与 `auction.settlement`。所有字段都可
/// 省略；应用层会在创建事务中把已校验模板同时写入 JSON 快照和遗留规则列。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuctionConfig {
    /// 竞价经济和生命周期参数。
    pub economics: AuctionEconomicsConfig,
    /// 结算曲线和恢复策略参数。
    pub settlement: AuctionSettlementConfig,
}

impl AuctionConfig {
    /// 将已通过主配置校验的 TOML 设置转换为应用层策略模板。
    pub fn policy(&self) -> AuctionPolicyTemplate {
        let settlement_mode = match self.settlement.mode.as_str() {
            "legacy_zero_v0" => SettlementMode::LegacyZeroV0,
            _ => SettlementMode::HoldCurveV1,
        };
        let recovery_mode = match self.settlement.recovery.mode.as_str() {
            "remind_v1" => RecoveryMode::RemindV1,
            _ => RecoveryMode::RemindV1,
        };
        AuctionPolicyTemplate {
            start_price: self.economics.start_price,
            bid_increment_ppm: self.economics.bid_increment_ppm,
            soft_close_window: Duration::seconds(self.economics.soft_close_window_seconds),
            soft_close_extension: Duration::seconds(self.economics.soft_close_extension_seconds),
            proxy_enabled: self.economics.proxy_enabled,
            proxy_default_mode: self.economics.proxy_default_mode.clone(),
            proxy_slow_interval: Duration::seconds(self.economics.proxy_slow_interval_seconds),
            proxy_max_outbid_uses: self.economics.proxy_max_outbid_uses,
            final_phase_lead: Duration::seconds(self.economics.final_phase_lead_seconds),
            cancel_cutoff: Duration::seconds(self.economics.cancel_cutoff_seconds),
            settlement: SettlementPolicy {
                version: self.settlement.version,
                mode: settlement_mode,
                compensation_pool_enabled: self.settlement.compensation_pool_enabled,
                isekai_carrier_credit: self.settlement.isekai_carrier_credit,
                anchors: self
                    .settlement
                    .anchors
                    .iter()
                    .map(|anchor| AdjustmentAnchor {
                        hold_ppm: anchor.hold_ppm,
                        adjustment_ppm: anchor.adjustment_ppm,
                    })
                    .collect(),
            },
            recovery: RecoveryPolicy {
                version: self.settlement.recovery.version,
                mode: recovery_mode,
                payment_timeout: Duration::seconds(
                    self.settlement.recovery.payment_timeout_seconds,
                ),
                reminder_limit: self.settlement.recovery.reminder_limit,
            },
        }
    }
}

/// 拍卖经济参数；创建拍卖时冻结到策略快照，并由竞价领域在运行时使用。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuctionEconomicsConfig {
    /// 默认起始价，必须为正且按 1000 信用网格对齐；省略时沿用旧版共享配置的 1000。
    pub start_price: i64,
    /// 经济策略的百万分比增量，范围 0-1000000；省略时为旧版共享配置的 50000。
    pub bid_increment_ppm: i64,
    /// 软截止窗口秒数，不能为负；省略时为 60 秒。
    pub soft_close_window_seconds: i64,
    /// 软截止延长秒数，必须大于 0；省略时为 60 秒。
    pub soft_close_extension_seconds: i64,
    /// 取消截止窗口秒数，不能为负；省略时为 86400 秒。
    pub cancel_cutoff_seconds: i64,
    /// 是否启用代理竞价；省略时启用。
    pub proxy_enabled: bool,
    /// 新代理授权默认模式，只能是 fast 或 slow；省略时为 fast。
    pub proxy_default_mode: String,
    /// 代理 slow 模式间隔秒数，必须大于 0；省略时为 600 秒。
    pub proxy_slow_interval_seconds: i64,
    /// 单个代理授权允许的 outbid 使用次数，省略时为 3；0 表示不限制。
    pub proxy_max_outbid_uses: u32,
    /// final phase 领先秒数，范围 1-604800；省略时为 60 秒。
    pub final_phase_lead_seconds: i64,
}

impl Default for AuctionEconomicsConfig {
    fn default() -> Self {
        Self {
            start_price: 1_000,
            bid_increment_ppm: 50_000,
            soft_close_window_seconds: 60,
            soft_close_extension_seconds: 60,
            cancel_cutoff_seconds: 86_400,
            proxy_enabled: true,
            proxy_default_mode: "fast".to_owned(),
            proxy_slow_interval_seconds: 600,
            proxy_max_outbid_uses: 3,
            final_phase_lead_seconds: 60,
        }
    }
}

/// 结算策略配置；锚点顺序和闭集模式由应用/领域校验。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuctionSettlementConfig {
    /// 结算策略版本；当前支持 0 或 1，省略时为 1。
    pub version: u32,
    /// 结算模式，只能是 hold_curve_v1 或 legacy_zero_v0；省略时为 hold_curve_v1。
    pub mode: String,
    /// 是否启用补偿池；省略时启用。
    pub compensation_pool_enabled: bool,
    /// isekai carrier credit 数量；当前策略要求为 1。
    pub isekai_carrier_credit: u32,
    /// 持有时长与调整比例的 PPM 锚点；省略时使用内置 hold curve。
    pub anchors: Vec<AuctionAdjustmentAnchorConfig>,
    /// 结算后的付款恢复策略。
    pub recovery: AuctionRecoveryPolicyConfig,
}

impl Default for AuctionSettlementConfig {
    fn default() -> Self {
        let policy = SettlementPolicy::default();
        Self {
            version: policy.version,
            mode: "hold_curve_v1".to_owned(),
            compensation_pool_enabled: policy.compensation_pool_enabled,
            isekai_carrier_credit: policy.isekai_carrier_credit,
            anchors: policy
                .anchors
                .into_iter()
                .map(|anchor| AuctionAdjustmentAnchorConfig {
                    hold_ppm: anchor.hold_ppm,
                    adjustment_ppm: anchor.adjustment_ppm,
                })
                .collect(),
            recovery: AuctionRecoveryPolicyConfig::default(),
        }
    }
}

/// 结算曲线中的一个整数锚点。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuctionAdjustmentAnchorConfig {
    /// 已连续领先时长的百万分比。
    pub hold_ppm: i64,
    /// 对最终金额应用的调整百万分比。
    pub adjustment_ppm: i64,
}

/// 恢复策略配置；当前领域仅接受七天、一次提醒的 remind_v1。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuctionRecoveryPolicyConfig {
    /// 恢复策略版本；省略时为 1。
    pub version: u32,
    /// 恢复模式；当前只能是 remind_v1。
    pub mode: String,
    /// 付款超时秒数；当前默认七天。
    pub payment_timeout_seconds: i64,
    /// 提醒次数；当前默认一次。
    pub reminder_limit: u32,
}

impl Default for AuctionRecoveryPolicyConfig {
    fn default() -> Self {
        Self {
            version: 1,
            mode: "remind_v1".to_owned(),
            payment_timeout_seconds: 7 * 24 * 60 * 60,
            reminder_limit: 1,
        }
    }
}

/// 进程本地 Prometheus 指标导出策略。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MetricsConfig {
    /// 是否注册并暴露固定 `/metrics` scrape endpoint；省略时关闭。
    pub enabled: bool,
}

/// Public/Worker 进程的 OTLP/HTTP tracing 非敏感运行参数。
///
/// Admin 进程在提供 runtime 文档时以其中的 `tracing` 为最终生效值；Public 和 Worker 使用
/// 主配置中的这一组字段。OTLP headers 属于敏感材料，必须放在 `secrets_file` 的对应字段中。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TracingSettings {
    /// 导出批次等待秒数；省略时为 2 秒，范围 1-30。
    pub batch_timeout_seconds: u64,
    /// 是否启用 OTLP/HTTP tracing 导出；省略时关闭。
    pub enabled: bool,
    /// OTLP/HTTP collector 地址；启用时必须是绝对 HTTP(S) 地址。
    pub endpoint: String,
    /// 单次导出超时秒数；省略时为 5 秒，范围 1-30。
    pub export_timeout_seconds: u64,
    /// 单次导出的最大 span 数；省略时为 64，范围 1-512 且不能超过队列容量。
    pub max_export_batch_size: u32,
    /// 导出队列容量；省略时为 256，范围 1-4096。
    pub queue_capacity: u32,
    /// 采样比例，单位为百万分率；启用时范围 1-1000000，关闭时允许 0。
    pub sample_ratio_ppm: u32,
    /// 进程关闭时等待导出完成的最大秒数；省略时为 5 秒，范围 1-30。
    pub shutdown_timeout_seconds: u64,
}

impl Default for TracingSettings {
    fn default() -> Self {
        Self {
            batch_timeout_seconds: 2,
            enabled: false,
            endpoint: String::new(),
            export_timeout_seconds: 5,
            max_export_batch_size: 64,
            queue_capacity: 256,
            sample_ratio_ppm: 0,
            shutdown_timeout_seconds: 5,
        }
    }
}

/// Public 进程 realtime relay 与连接 Hub 的非敏感策略。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RealtimeConfig {
    /// Public durable consumer 的稳定标识；多 Public 实例必须各自使用不同值。
    pub consumer_key: String,
    /// relay 两次扫描之间的毫秒数；省略时为 250 毫秒。
    pub poll_interval_millis: u64,
    /// 单轮最多扫描的 stream 数；省略时为 200。
    pub stream_batch_size: u16,
    /// 单个 stream 单轮最多转发的事件数；省略时为 200。
    pub event_batch_size: u16,
    /// 每个 WebSocket 连接的进程内有界缓冲数；省略时为 32。
    pub client_buffer: usize,
}

impl Default for RealtimeConfig {
    fn default() -> Self {
        Self {
            consumer_key: "public-1".to_owned(),
            poll_interval_millis: 250,
            stream_batch_size: 200,
            event_batch_size: 200,
            client_buffer: 32,
        }
    }
}
