//! 进程 TOML 配置与敏感配置加载。
//!
//! 普通配置负责监听地址、缓存模式等非敏感设置；主配置可通过 `secrets_file` 引用独立的
//! 敏感 TOML。任何凭据都必须通过显式 getter 使用，不能依赖 `Debug` 输出或环境变量覆盖。

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fmt, fs, io,
    io::Read,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    str::FromStr,
};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

use chrono::{Duration, Utc};
use ipnet::IpNet;
use serde::Deserialize;
use thiserror::Error;
use url::Url;

use auction_application::{
    AdminRuntimeDocument, AuctionPolicyTemplate, OperationsAlertPolicy, OperationsAlertTarget,
    RecurringSchedule,
};
use auction_domain::{
    AdjustmentAnchor, RecoveryMode, RecoveryPolicy, SettlementMode, SettlementPolicy,
};

use crate::StandardCronSchedule;

const CONFIG_FILE_ENV: &str = "AUCTION_CONFIG_FILE";

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

    const fn default_listen(self) -> Option<&'static str> {
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

/// Auction 主循环及其它兼容轮询 owner 的共享调度边界。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkerConfig {
    /// Worker control Unix socket 配置；默认关闭，避免无权限环境启动失败。
    pub control: WorkerControlConfig,
    /// Auction 主循环并发数；省略时 PostgreSQL 为 8、SQLite Lite 为 1。
    pub concurrency: Option<u16>,
    /// Auction 主循环及兼容 owner 的轮询间隔秒数；省略时为 60 秒。
    pub poll_interval_seconds: Option<u64>,
    /// Auction 生命周期和结算各自的单轮批次上限；范围 1-1000，默认 100。
    pub batch_limit: u16,
    /// 每轮完成后附加的随机抖动上限秒数；范围 0-86400，默认 5。
    pub jitter_seconds: u64,
    /// Auction 单个生命周期或结算任务的总处理超时秒数；默认 30。
    pub item_timeout_seconds: u64,
    /// Recovery 各阶段的持久化重试间隔秒数；省略时为 60 秒，最大不超过 86400 秒。
    pub recovery_retry_delay_seconds: u64,
    /// Delivery loop 的独立批次、lease 和超时边界。
    pub delivery: DeliveryWorkerConfig,
    /// Fulfillment loop 的独立批次、并发、lease 和超时边界。
    pub fulfillment: FulfillmentWorkerConfig,
    /// Recovery loop 的独立批次、并发、lease 和超时边界。
    pub recovery: RecoveryWorkerConfig,
    /// 可选 MoogleMail 轮询配置；省略时关闭远程读取。
    pub mooglemail: MoogleMailWorkerConfig,
    /// 可选 E-Hentai 论坛主题同步配置；省略时关闭远程写入。
    pub forum: ForumSyncWorkerConfig,
    /// 可选 operations alert 运维告警配置；省略时关闭告警扫描与 MoogleMail 通知。
    pub operations_alert: OperationsAlertWorkerConfig,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            control: WorkerControlConfig::default(),
            concurrency: None,
            poll_interval_seconds: None,
            batch_limit: 100,
            jitter_seconds: 5,
            item_timeout_seconds: 30,
            recovery_retry_delay_seconds: 60,
            delivery: DeliveryWorkerConfig::default(),
            fulfillment: FulfillmentWorkerConfig::default(),
            recovery: RecoveryWorkerConfig::default(),
            mooglemail: MoogleMailWorkerConfig::default(),
            forum: ForumSyncWorkerConfig::default(),
            operations_alert: OperationsAlertWorkerConfig::default(),
        }
    }
}

/// Delivery outbox 的独立领取、续租与外部调用边界。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DeliveryWorkerConfig {
    /// 单轮最多领取的 delivery 数量；范围 1-1000，默认 100。
    pub batch_limit: u16,
    /// 单轮 delivery 并发数；范围 1-batch_limit，默认 8。
    pub concurrency: u16,
    /// Delivery 独立轮询间隔秒数；范围 1-86400，默认 60。
    pub poll_interval_seconds: u64,
    /// 每轮完成后附加的随机抖动上限秒数；范围 0-86400，默认 5。
    pub jitter_seconds: u64,
    /// claim lease 秒数；范围 1-86400，默认 60。
    pub lease_duration_seconds: u64,
    /// sending 期间续租间隔秒数；必须小于 lease，默认 10。
    pub renew_interval_seconds: u64,
    /// lease 过期后等待外部结果写回的宽限秒数；必须大于请求超时，默认 120。
    pub sending_grace_seconds: u64,
    /// 单次完整 DeliveryTransport 调用总超时秒数；默认 30。
    pub request_timeout_seconds: u64,
    /// 成功、失败或人工对账结果写回的超时秒数；默认 5。
    pub finalize_timeout_seconds: u64,
}

impl Default for DeliveryWorkerConfig {
    fn default() -> Self {
        Self {
            batch_limit: 100,
            concurrency: 8,
            poll_interval_seconds: 60,
            jitter_seconds: 5,
            lease_duration_seconds: 60,
            renew_interval_seconds: 10,
            sending_grace_seconds: 120,
            request_timeout_seconds: 30,
            finalize_timeout_seconds: 5,
        }
    }
}

/// Settlement fulfillment Worker 的独立领取与处理边界。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FulfillmentWorkerConfig {
    /// 单轮最多处理的 fulfillment 数量；范围 1-1000，默认 100。
    pub batch_limit: u16,
    /// fulfillment action wave 的并发数；范围 1-batch_limit，默认 8。
    pub concurrency: u16,
    /// 两轮处理之间的固定轮询秒数；范围 1-86400，默认 60。
    pub poll_interval_seconds: u64,
    /// 每轮完成后附加的随机抖动上限秒数；范围 0-86400，默认 5。
    pub jitter_seconds: u64,
    /// fulfillment claim lease 秒数；范围 1-86400，默认 60。
    pub lease_duration_seconds: u64,
    /// 单个 fulfillment 的总处理超时秒数；必须小于 lease，默认 30。
    pub item_timeout_seconds: u64,
    /// 依赖暂不可用后的持久化重试间隔秒数；范围 1-86400，默认 60。
    pub retry_delay_seconds: u64,
}

impl Default for FulfillmentWorkerConfig {
    fn default() -> Self {
        Self {
            batch_limit: 100,
            concurrency: 8,
            poll_interval_seconds: 60,
            jitter_seconds: 5,
            lease_duration_seconds: 60,
            item_timeout_seconds: 30,
            retry_delay_seconds: 60,
        }
    }
}

/// Recovery Worker 的独立调度和批次边界。
///
/// 旧版 recovery loop 使用独立的 `batch_limit` 和 `concurrency`，不能复用拍卖主循环的
/// 全局并发，否则一个高峰期会同时放大退回、action、重列和退款处理量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RecoveryWorkerConfig {
    /// 每个 recovery phase 单轮最多领取的记录数；范围 1-1000，默认 100。
    pub batch_limit: u16,
    /// recovery action wave 的并发数；范围 1-batch_limit，默认 8。
    pub concurrency: u16,
    /// 两轮五阶段处理之间的固定轮询秒数；范围 1-86400，默认 60。
    pub poll_interval_seconds: u64,
    /// 每轮完成后附加的随机抖动上限秒数；范围 0-86400，默认 5。
    pub jitter_seconds: u64,
    /// recovery action claim lease 秒数；范围 1-86400，默认 60。
    pub lease_duration_seconds: u64,
    /// 单个 recovery 记录或 action 的总处理超时秒数；必须小于 lease，默认 30。
    pub item_timeout_seconds: u64,
    /// 余额低水位阈值；低于该值时写入 `low`，默认 100 credits。
    pub low_balance_threshold: i64,
    /// recovery health 两次远端余额读取之间的最小间隔秒数；默认 60 秒。
    pub health_refresh_interval_seconds: u64,
    /// 外部余额证据超过该年龄后标记为 stale；必须不小于刷新间隔，默认 300 秒。
    pub evidence_stale_after_seconds: u64,
}

impl Default for RecoveryWorkerConfig {
    fn default() -> Self {
        Self {
            batch_limit: 100,
            concurrency: 8,
            poll_interval_seconds: 60,
            jitter_seconds: 5,
            lease_duration_seconds: 60,
            item_timeout_seconds: 30,
            low_balance_threshold: 100,
            health_refresh_interval_seconds: 60,
            evidence_stale_after_seconds: 300,
        }
    }
}

/// Worker 使用的 operations alert 扫描、阈值和通知批次配置。
///
/// 所有字段都有默认值，因此 TOML 可以只覆盖需要调整的字段；启用告警时必须显式提供
/// 合法的 `recipient`。通知正文只由应用层从健康摘要生成，不接受配置注入。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OperationsAlertWorkerConfig {
    /// 是否启用运维告警扫描和 MoogleMail 通知；省略时关闭。
    pub enabled: bool,
    /// scan、dispatch 与 reconcile 各自的轮询间隔秒数；范围 1-86400，省略时为 60 秒。
    pub scan_interval_seconds: u64,
    /// 读取四类健康来源的超时秒数，范围 1-30；省略时为 5 秒。
    pub source_timeout_seconds: u64,
    /// 同一轮观察结果合并到通知批次前的窗口秒数，范围 1-3600；省略时为 120 秒。
    pub batch_window_seconds: u64,
    /// 同一告警 signal 的通知冷却秒数，不能小于批次窗口；省略时为 1800 秒。
    pub cooldown_seconds: u64,
    /// 告警批次 claim 租约秒数，必须覆盖来源读取超时；省略时为 120 秒。
    pub claim_lease_seconds: u64,
    /// 告警投递失败后的持久化重试间隔秒数；省略时为 300 秒。
    pub retry_delay_seconds: u64,
    /// 单轮最多处理的 signal 数量，范围 1-64；省略时为 64。
    pub max_signals_per_scan: usize,
    /// 单个 MoogleMail 通知批次最多包含的 signal 数量，范围 1-64；省略时为 20。
    pub max_batch_items: usize,
    /// 单个 MoogleMail 通知正文的最大字节数，范围 256-16384；省略时为 4096。
    pub max_body_bytes: usize,
    /// delivery pending 数量阈值；达到后产生告警，省略时为 1。
    pub delivery_pending_count_threshold: i64,
    /// delivery pending 最老记录年龄阈值秒数，范围 1-2592000；省略时为 900 秒。
    pub delivery_pending_age_seconds: u64,
    /// delivery manual_reconcile 数量阈值；达到后产生告警，省略时为 1。
    pub delivery_manual_reconcile_count_threshold: i64,
    /// settlement fulfillment backlog 数量阈值；达到后产生告警，省略时为 1。
    pub fulfillment_backlog_count_threshold: i64,
    /// settlement fulfillment backlog 最老记录年龄阈值秒数，省略时为 900 秒。
    pub fulfillment_backlog_age_seconds: u64,
    /// recovery payment overdue 数量阈值；达到后产生告警，省略时为 1。
    pub recovery_overdue_count_threshold: i64,
    /// recovery payment overdue 最老记录年龄阈值秒数，省略时为 900 秒。
    pub recovery_overdue_age_seconds: u64,
    /// 运维告警通知使用的游戏世界，只允许 main 或 isekai；省略时为 main。
    pub world_id: String,
    /// 运维告警通知收件人；告警启用时必填，省略时为空字符串。
    pub recipient: String,
}

impl Default for OperationsAlertWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            scan_interval_seconds: 60,
            source_timeout_seconds: 5,
            batch_window_seconds: 2 * 60,
            cooldown_seconds: 30 * 60,
            claim_lease_seconds: 120,
            retry_delay_seconds: 5 * 60,
            max_signals_per_scan: 64,
            max_batch_items: 20,
            max_body_bytes: 4 << 10,
            delivery_pending_count_threshold: 1,
            delivery_pending_age_seconds: 15 * 60,
            delivery_manual_reconcile_count_threshold: 1,
            fulfillment_backlog_count_threshold: 1,
            fulfillment_backlog_age_seconds: 15 * 60,
            recovery_overdue_count_threshold: 1,
            recovery_overdue_age_seconds: 15 * 60,
            world_id: "main".to_owned(),
            recipient: String::new(),
        }
    }
}

impl OperationsAlertWorkerConfig {
    /// 将已校验的 TOML 设置转换为应用层告警策略。
    pub fn policy(&self) -> OperationsAlertPolicy {
        OperationsAlertPolicy {
            source_timeout: std::time::Duration::from_secs(self.source_timeout_seconds),
            batch_window: std::time::Duration::from_secs(self.batch_window_seconds),
            cooldown: std::time::Duration::from_secs(self.cooldown_seconds),
            claim_lease: std::time::Duration::from_secs(self.claim_lease_seconds),
            retry_delay: std::time::Duration::from_secs(self.retry_delay_seconds),
            max_signals_per_scan: self.max_signals_per_scan,
            max_batch_items: self.max_batch_items,
            max_body_bytes: self.max_body_bytes,
            delivery_pending_count_threshold: self.delivery_pending_count_threshold,
            delivery_pending_age_threshold: std::time::Duration::from_secs(
                self.delivery_pending_age_seconds,
            ),
            delivery_manual_reconcile_count_threshold: self
                .delivery_manual_reconcile_count_threshold,
            fulfillment_backlog_count_threshold: self.fulfillment_backlog_count_threshold,
            fulfillment_backlog_age_threshold: std::time::Duration::from_secs(
                self.fulfillment_backlog_age_seconds,
            ),
            recovery_overdue_count_threshold: self.recovery_overdue_count_threshold,
            recovery_overdue_age_threshold: std::time::Duration::from_secs(
                self.recovery_overdue_age_seconds,
            ),
        }
    }

    /// 将已校验的 TOML 设置转换为应用层告警收件目标。
    pub fn target(&self) -> OperationsAlertTarget {
        OperationsAlertTarget { world_id: self.world_id.clone(), recipient: self.recipient.clone() }
    }
}

/// Worker control Unix socket 的非敏感配置。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkerControlConfig {
    /// 是否启用 Worker control；省略时关闭。
    pub enabled: bool,
    /// Unix socket 所属组名；省略时使用旧版默认组 `auction-worker-control`。
    pub group: String,
    /// 允许访问控制面的内核认证 GID 白名单；省略时为空。
    pub allowed_peer_gids: Vec<u32>,
    /// 允许访问控制面的内核认证 UID 白名单；省略时为空。
    pub allowed_peer_uids: Vec<u32>,
    /// 预先建立的规范化绝对运行目录；省略时使用 `/run/hv-auction/worker`。
    pub runtime_directory: String,
    /// 规范化绝对 socket 路径；省略时使用 `/run/hv-auction/worker/control.sock`。
    pub socket_path: String,
}

impl Default for WorkerControlConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            group: "auction-worker-control".to_owned(),
            allowed_peer_gids: Vec::new(),
            allowed_peer_uids: Vec::new(),
            runtime_directory: "/run/hv-auction/worker".to_owned(),
            socket_path: "/run/hv-auction/worker/control.sock".to_owned(),
        }
    }
}

/// Worker 使用的 E-Hentai 论坛主题同步策略。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ForumSyncWorkerConfig {
    /// 是否启用论坛主题创建/幂等查找；默认关闭，避免配置不完整时触发远程 POST。
    pub enabled: bool,
    /// active ForumCookie 凭据所属的 world scope；允许 main、isekai 或 shared。
    pub credential_world: String,
    /// 可选固定论坛 endpoint；省略时使用 `https://forums.e-hentai.org`。
    pub endpoint: Option<String>,
    /// 可选公开端 origin；配置后论坛主题会附带指向拍卖页面的绝对链接。
    pub public_base_url: Option<String>,
    /// 是否把 main/isekai 都映射到旧版测试板块 4；生产环境必须保持 false。
    pub test_board_mode: bool,
    /// 单轮最多领取的主题同步任务数，范围 1-100。
    pub batch_size: u16,
    /// 单次响应最大字节数，范围 1-8 MiB。
    pub maximum_response_bytes: usize,
    /// 连续论坛请求之间的最小间隔毫秒数。
    pub minimum_request_interval_millis: u64,
    /// 单次论坛 HTTP 请求超时秒数，范围 1-300。
    pub request_timeout_seconds: u64,
    /// 单个论坛同步任务的总处理超时秒数，必须小于 Worker 租约。
    pub item_timeout_seconds: u64,
    /// 单个论坛发布 claim 的租约秒数；每个主题在实际领取时单独计算。
    pub lease_duration_seconds: u64,
}

impl Default for ForumSyncWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            credential_world: "shared".to_owned(),
            endpoint: None,
            public_base_url: None,
            test_board_mode: false,
            batch_size: 100,
            maximum_response_bytes: 2 * 1024 * 1024,
            minimum_request_interval_millis: 750,
            request_timeout_seconds: 30,
            item_timeout_seconds: 93,
            lease_duration_seconds: 300,
        }
    }
}

/// Worker 使用的 MoogleMail 入站轮询策略。
///
/// `equipment_filter` 与 `mail_templates` 等旧版 runtime 业务策略随 Worker 配置在启动时
/// 读取；它们只影响入站业务判断和通知内容，不包含 Cookie、密钥或其他敏感值。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MoogleMailWorkerConfig {
    /// 是否启用远程 MoogleMail 入站读取；默认关闭，避免配置不完整时发出外部请求。
    pub enabled: bool,
    /// 持久化邮箱标识；同一邮箱只能由一个有效租约并行处理。
    pub mailbox_key: String,
    /// 游戏世界，只允许 `main` 或 `isekai`。
    pub world_id: String,
    /// MoogleMail 拍卖邮件使用的起拍价覆盖值；省略时继承 `[auction.economics].start_price`。
    pub start_price: Option<i64>,
    /// MoogleMail 拍卖装备过滤策略；省略时关闭过滤但仍要求来源标记为可交易。
    pub equipment_filter: MoogleMailEquipmentFilterConfig,
    /// MoogleMail 业务通知模板；省略时使用旧版默认英文模板。
    pub mail_templates: MoogleMailNotificationTemplatesConfig,
    /// 可选 `.keyenv` 路径；相对路径相对于主 Worker 配置文件解析。
    pub keyenv_file: Option<PathBuf>,
    /// MoogleMail 固定生产 origin 或显式回环联调 origin。
    pub endpoint: String,
    /// 单次 MoogleMail 响应最大字节数，不能超过 8 MiB。
    pub maximum_response_bytes: usize,
    /// 同一客户端连续远端请求的最小间隔毫秒数。
    pub minimum_request_interval_millis: u64,
    /// 单次 MoogleMail HTTP 请求与响应正文读取的超时秒数。
    pub request_timeout_seconds: u64,
    /// 单次从 MoogleMail 读取的最大邮件数。
    pub batch_size: u16,
    /// 单次有界分页最多读取的页数。
    pub maximum_pages: usize,
    /// 每个 world 共用的标准五字段 cron；支持旧版描述符和时区前缀。
    pub schedule: String,
    /// 每个 cron 触发时刻后附加的随机抖动上限秒数。
    pub jitter_seconds: u64,
    /// 邮箱 claim 租约秒数；长批次处理期间会周期续租。
    pub lease_duration_seconds: u64,
    /// 邮箱 claim 续租周期秒数，必须小于租约。
    pub renew_interval_seconds: u64,
    /// 一轮远程读取的内联尝试次数。
    pub fetch_attempts: u16,
    /// 每次远程读取尝试的总超时秒数。
    pub fetch_timeout_seconds: u64,
    /// 单封邮件业务处理的内联尝试次数。
    pub process_attempts: u16,
    /// 单次业务处理尝试的总超时秒数。
    pub item_timeout_seconds: u64,
    /// 内联与持久化指数退避的基础秒数。
    pub retry_base_seconds: u64,
    /// 内联与持久化指数退避的最大秒数。
    pub retry_maximum_seconds: u64,
    /// 发件人 allowlist；未显式关闭 allowlist 时只读取其中的详情。
    pub sender_allowlist: Vec<String>,
    /// 明确关闭发件人 allowlist 后才读取所有发件人详情。
    pub sender_allowlist_disabled: bool,
    /// 额外启用的 MoogleMail 世界；每个条目拥有独立邮箱游标和发件人策略。
    ///
    /// 顶层字段继续描述第一个世界，以保持现有单世界配置可用；新增世界通过
    /// `[[worker.mooglemail.additional_worlds]]` 声明。每个条目的存在即表示该世界启用。
    pub additional_worlds: Vec<MoogleMailWorldConfig>,
}

/// MoogleMail 业务通知模板配置。
///
/// 模板键与旧版 runtime 的闭集保持一致；字段采用结构化配置而不是自由键值表，避免
/// 拼写错误或把未审计的占位符带入远端邮件。模板正文只允许 `{name}` 形式的已知占位符。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MoogleMailNotificationTemplatesConfig {
    /// 拍卖邮件包含被拒绝资产时的正文。
    pub auction_assets_rejected: String,
    /// 拍卖邮件没有附件时的正文。
    pub auction_no_attachments: String,
    /// 拍卖装备不符合条件时的正文；支持 `{reason_code}`。
    pub auction_equipment_rejected: String,
    /// 用户邮件包含拍卖资产时的正文。
    pub user_assets_rejected: String,
    /// 注册成功或恢复成功时发送登录 key；支持 `{login_key}`、`{username}`。
    pub register_key: String,
    /// 注册结果通知；支持 `{result_code}`、`{username}`。
    pub register_result: String,
    /// 重绑结果通知；支持 `{result_code}`、`{username}`。
    pub rebind_result: String,
    /// 主世界竞得装备并使用货到付款时的正文；支持 `{item_name}`、`{amount}`、`{reference}`。
    pub settlement_winner_equipment_cod: String,
    /// 异世界竞得付款载体时的正文；支持 `{amount}`、`{reference}`。
    pub settlement_payment_carrier: String,
    /// 结算付款超时时的正文；支持 `{reference}`。
    pub settlement_payment_reminder: String,
    /// 结算装备交付或退回时的正文；支持 `{item_name}`、`{reference}`。
    pub settlement_equipment_handoff: String,
    /// 卖家收款时的正文；支持 `{amount}`、`{reference}`。
    pub settlement_seller_payout: String,
    /// 买家退款时的正文；支持 `{amount}`、`{reference}`。
    pub settlement_buyer_refund: String,
    /// 运维告警正文；支持 `{summary}`。
    pub operations_alert: String,
}

impl Default for MoogleMailNotificationTemplatesConfig {
    fn default() -> Self {
        Self {
            auction_assets_rejected: "Auction assets rejected".to_owned(),
            auction_no_attachments: "Auction has no attachments".to_owned(),
            auction_equipment_rejected: "Auction equipment rejected: {reason_code}".to_owned(),
            user_assets_rejected: "User assets rejected".to_owned(),
            register_key: "Registration key {login_key} for {username}".to_owned(),
            register_result: "Registration result {result_code} {username}".to_owned(),
            rebind_result: "Rebind result {result_code} {username}".to_owned(),
            settlement_winner_equipment_cod:
                "Auction item {item_name}; COD {amount}; reference {reference}".to_owned(),
            settlement_payment_carrier: "Auction payment request {amount}; reference {reference}"
                .to_owned(),
            settlement_payment_reminder: "Auction payment is overdue; reference {reference}"
                .to_owned(),
            settlement_equipment_handoff: "Auction item {item_name}; reference {reference}"
                .to_owned(),
            settlement_seller_payout: "Auction seller payout {amount}; reference {reference}"
                .to_owned(),
            settlement_buyer_refund: "Auction buyer refund {amount}; reference {reference}"
                .to_owned(),
            operations_alert: "Operations alert: {summary}".to_owned(),
        }
    }
}

impl MoogleMailNotificationTemplatesConfig {
    /// 转换为 transport 启动时使用的稳定模板快照。
    pub fn values(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("auction_assets_rejected".to_owned(), self.auction_assets_rejected.clone()),
            ("auction_no_attachments".to_owned(), self.auction_no_attachments.clone()),
            ("auction_equipment_rejected".to_owned(), self.auction_equipment_rejected.clone()),
            ("user_assets_rejected".to_owned(), self.user_assets_rejected.clone()),
            ("register_key".to_owned(), self.register_key.clone()),
            ("register_result".to_owned(), self.register_result.clone()),
            ("rebind_result".to_owned(), self.rebind_result.clone()),
            (
                "settlement_winner_equipment_cod".to_owned(),
                self.settlement_winner_equipment_cod.clone(),
            ),
            ("settlement_payment_carrier".to_owned(), self.settlement_payment_carrier.clone()),
            ("settlement_payment_reminder".to_owned(), self.settlement_payment_reminder.clone()),
            ("settlement_equipment_handoff".to_owned(), self.settlement_equipment_handoff.clone()),
            ("settlement_seller_payout".to_owned(), self.settlement_seller_payout.clone()),
            ("settlement_buyer_refund".to_owned(), self.settlement_buyer_refund.clone()),
            ("operations_alert".to_owned(), self.operations_alert.clone()),
        ])
    }
}

/// MoogleMail 拍卖装备过滤策略。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MoogleMailEquipmentFilterConfig {
    /// 是否启用品质、前后缀、分类、类型和等级过滤；默认关闭。
    pub enabled: bool,
    /// 允许的品质 token；空数组表示不限制。
    pub qualities: Vec<String>,
    /// 允许的前缀 token；空数组表示不限制。
    pub prefixes: Vec<String>,
    /// 允许的后缀 token；空数组表示不限制。
    pub suffixes: Vec<String>,
    /// 允许的装备分类 token；空数组表示不限制。
    pub categories: Vec<String>,
    /// 允许的装备类型 token；空数组表示不限制。
    pub equipment_types: Vec<String>,
    /// 最低装备等级；0 表示不启用等级下限。
    pub level_min: i32,
    /// 最高装备等级；0 表示不启用等级上限，最大允许 1000。
    pub level_max: i32,
}

/// 单个 MoogleMail 世界的入站轮询配置。
///
/// 世界配置不包含 Cookie 或其他敏感值。Worker 会为每个世界建立独立的持久化邮箱状态，
/// 但复用同一组已校验的外部凭据池。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MoogleMailWorldConfig {
    /// 游戏世界，只允许 `main` 或 `isekai`。
    pub world_id: String,
    /// 该世界对应的持久化邮箱标识；必须与其他世界不同。
    pub mailbox_key: String,
    /// 该世界允许读取详情的发件人集合。
    pub sender_allowlist: Vec<String>,
    /// 明确设为 true 才读取该世界的所有发件人详情。
    pub sender_allowlist_disabled: bool,
}

impl Default for MoogleMailWorldConfig {
    fn default() -> Self {
        Self {
            world_id: "isekai".to_owned(),
            mailbox_key: "mooglemail:isekai".to_owned(),
            sender_allowlist: Vec::new(),
            sender_allowlist_disabled: false,
        }
    }
}

impl Default for MoogleMailWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mailbox_key: "mooglemail:main".to_owned(),
            world_id: "main".to_owned(),
            start_price: None,
            equipment_filter: MoogleMailEquipmentFilterConfig::default(),
            mail_templates: MoogleMailNotificationTemplatesConfig::default(),
            keyenv_file: None,
            endpoint: "https://hentaiverse.org".to_owned(),
            maximum_response_bytes: 8 * 1024 * 1024,
            minimum_request_interval_millis: 1_000,
            request_timeout_seconds: 15,
            batch_size: 100,
            maximum_pages: 20,
            schedule: "* * * * *".to_owned(),
            jitter_seconds: 5,
            lease_duration_seconds: 60,
            renew_interval_seconds: 10,
            fetch_attempts: 3,
            fetch_timeout_seconds: 30,
            process_attempts: 3,
            item_timeout_seconds: 30,
            retry_base_seconds: 1,
            retry_maximum_seconds: 30,
            sender_allowlist: Vec::new(),
            sender_allowlist_disabled: false,
            additional_worlds: Vec::new(),
        }
    }
}

impl MoogleMailWorkerConfig {
    /// 返回按 `main`、`isekai` 稳定排序的全部已配置世界。
    pub fn configured_worlds(&self) -> Vec<MoogleMailWorldConfig> {
        let mut worlds = Vec::with_capacity(1 + self.additional_worlds.len());
        worlds.push(MoogleMailWorldConfig {
            world_id: self.world_id.clone(),
            mailbox_key: self.mailbox_key.clone(),
            sender_allowlist: self.sender_allowlist.clone(),
            sender_allowlist_disabled: self.sender_allowlist_disabled,
        });
        worlds.extend(self.additional_worlds.iter().cloned());
        worlds.sort_by_key(|world| if world.world_id == "main" { 0 } else { 1 });
        worlds
    }
}

/// 浏览器写请求和公开会话使用的非敏感 HTTP 安全策略。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HttpSecurityConfig {
    /// 允许的精确 HTTP(S) origin；启用 allowlist 时必须填写。
    pub allowed_origins: Vec<String>,
    /// 是否启用 origin allowlist；省略时关闭。
    pub allowed_origins_enabled: bool,
    /// 登录请求在一个窗口内的最大次数；省略时为 10 次。
    pub login_rate_limit: u32,
    /// 状态变更请求在一个窗口内的最大次数；省略时为 60 次。
    pub mutation_rate_limit: u32,
    /// 速率限制窗口秒数；省略时为 60 秒。
    pub rate_limit_window_seconds: u64,
    /// 是否为浏览器 Cookie 设置 Secure；省略时关闭，生产环境应开启。
    pub cookie_secure: bool,
    /// 普通 HTTP 请求的最长处理秒数；省略时为 15 秒，WebSocket 握手路径不受此限制。
    pub request_timeout_seconds: u64,
    /// 可以提供 `X-Forwarded-For` 的可信反向代理网段；省略时不信任任何 TCP 代理。
    pub trusted_proxy_cidrs: Vec<String>,
    /// 是否信任 Unix socket 上游提供的 `X-Forwarded-For`；省略时关闭，仅允许 Public/Admin
    /// Unix 监听器启用。
    pub trusted_unix_proxy: bool,
}

impl Default for HttpSecurityConfig {
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

/// Public 会话的滑动过期、绝对过期和并发上限，单位为秒。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionConfig {
    /// 会话空闲过期秒数；省略时为 1800 秒。
    pub idle_ttl_seconds: u64,
    /// 会话绝对过期秒数；省略时为 604800 秒。
    pub absolute_ttl_seconds: u64,
    /// 单个公开用户允许的最大并发会话数；省略时为 5 个。
    pub max_public_sessions: u32,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            idle_ttl_seconds: 30 * 60,
            absolute_ttl_seconds: 7 * 24 * 60 * 60,
            max_public_sessions: 5,
        }
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("unknown process role: {0}")]
    UnknownRole(String),

    #[error("failed to read TOML configuration {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    // 旧主配置可能仍含连接串或 Cookie，拒绝时也不能让解析器回显原始行。
    #[error("failed to parse TOML configuration {path}; configuration values are redacted")]
    Parse { path: PathBuf },

    #[error("failed to read sensitive TOML configuration {path}: {source}")]
    SecretRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    // 敏感 TOML 的解析器错误可能带出原始行，因此这里只报告文件路径。
    #[error("failed to parse sensitive TOML configuration {path}; secret values are redacted")]
    SecretParse { path: PathBuf },

    #[error("PostgreSQL maintenance sensitive TOML path is invalid: {path}")]
    InvalidMaintenanceSecretPath { path: PathBuf },

    #[error("PostgreSQL maintenance sensitive TOML must be a regular non-symlink file: {path}")]
    InvalidMaintenanceSecretFile { path: PathBuf },

    #[error("PostgreSQL maintenance sensitive TOML must be owner-only readable: {path}")]
    InsecureMaintenanceSecretPermissions { path: PathBuf },

    #[error(
        "PostgreSQL maintenance sensitive TOML parent must not allow group/other writes: {path}"
    )]
    InsecureMaintenanceSecretParent { path: PathBuf },

    #[error("failed to read local MoogleMail key environment {path}: {source}")]
    MoogleMailKeyEnvRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("local MoogleMail key environment {path} must be owner-only readable")]
    MoogleMailKeyEnvInsecurePermissions { path: PathBuf },

    #[error("failed to parse local MoogleMail key environment {path}; secret values are redacted")]
    MoogleMailKeyEnvParse { path: PathBuf },

    #[error("local MoogleMail key environment {path} is missing {key}")]
    MoogleMailKeyEnvMissing { path: PathBuf, key: &'static str },

    #[error("invalid {field} value {value:?}: {source}")]
    InvalidAddress { field: &'static str, value: String, source: std::net::AddrParseError },

    #[error("sensitive field {field} must not be empty")]
    EmptySensitiveField { field: &'static str },

    #[error("cache backend redis requires sensitive field redis_url")]
    MissingRedisUrl,

    #[error("invalid cache configuration: {0}")]
    InvalidCacheConfig(&'static str),

    #[error("invalid HTTP listener configuration: {0}")]
    InvalidListenerConfig(&'static str),

    #[error("equipment data key requires both equipment_data_key_id and equipment_data_key")]
    InvalidEquipmentDataKeyConfiguration,

    #[error("user login data key requires both user_login_data_key_id and user_login_data_key")]
    InvalidUserLoginDataKeyConfiguration,

    #[error("credential data key requires both credential_data_key_id and credential_data_key")]
    InvalidCredentialDataKeyConfiguration,

    #[error("worker concurrency must be greater than zero")]
    InvalidWorkerConcurrency,

    #[error("worker poll interval must be greater than zero")]
    InvalidWorkerPollInterval,

    #[error("worker recovery retry delay must be between 1 and 86400 seconds")]
    InvalidWorkerRecoveryRetryDelay,

    #[error("invalid auction Worker configuration: {0}")]
    InvalidAuctionWorkerConfig(&'static str),

    #[error("invalid delivery Worker configuration: {0}")]
    InvalidDeliveryWorkerConfig(&'static str),

    #[error("invalid fulfillment Worker configuration: {0}")]
    InvalidFulfillmentWorkerConfig(&'static str),

    #[error("invalid recovery Worker configuration: {0}")]
    InvalidRecoveryWorkerConfig(&'static str),

    #[error("invalid Worker control configuration: {0}")]
    InvalidWorkerControlConfig(&'static str),

    #[error(
        "enabled MoogleMail Worker requires keyenv_file, sensitive mooglemail_cookie, or credential data key"
    )]
    MissingMoogleMailCredentials,

    #[error("enabled MoogleMail Worker requires user_login_data_key_id and user_login_data_key")]
    MissingMoogleMailUserLoginDataKey,

    #[error("invalid MoogleMail Worker configuration: {0}")]
    InvalidMoogleMailConfig(&'static str),

    #[error("invalid auction policy configuration: {0}")]
    InvalidAuctionConfig(&'static str),

    #[error(
        "enabled E-Hentai forum Worker requires sensitive ehentai_cookie or credential data key"
    )]
    MissingEhentaiCredentials,

    #[error("invalid E-Hentai forum Worker configuration: {0}")]
    InvalidForumConfig(&'static str),

    #[error("invalid operations alert Worker configuration: {0}")]
    InvalidOperationsAlertConfig(&'static str),

    #[error("invalid realtime configuration: {0}")]
    InvalidRealtimeConfig(&'static str),

    #[error("invalid tracing configuration: {0}")]
    InvalidTracingConfig(&'static str),

    #[error("SQLite Lite requires worker concurrency = 1, got {concurrency}")]
    LiteWorkerConcurrency { concurrency: u16 },

    #[error("invalid HTTP security configuration: {0}")]
    InvalidHttpSecurity(&'static str),

    #[error("invalid session configuration: {0}")]
    InvalidSessionConfig(&'static str),

    #[error("failed to read Admin runtime TOML configuration {path}: {source}")]
    RuntimeRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("failed to parse Admin runtime TOML configuration {path}; values are redacted")]
    RuntimeParse { path: PathBuf },

    #[error("invalid Admin runtime configuration {path}: {reason}")]
    InvalidRuntimeConfig { path: PathBuf, reason: String },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    /// 兼容旧格式的可选监听地址；与 `listener` 同时提供会失败，建议使用完整 listener 区块。
    #[serde(default)]
    listen_address: Option<String>,
    /// 可选 HTTP listener 区块；Public/Admin 可选择 TCP 或 Linux Unix socket。
    #[serde(default)]
    listener: Option<HttpListenerConfig>,
    /// 可选 tracing 过滤器；省略时由装配根采用默认过滤器。
    #[serde(default)]
    log_filter: Option<String>,
    /// 可选 metrics 区块；省略时关闭进程本地 Prometheus 导出。
    #[serde(default)]
    metrics: MetricsConfig,
    /// 可选 tracing 区块；Public/Worker 使用，Admin 有 runtime 文件时由 runtime 覆盖。
    #[serde(default)]
    tracing: TracingSettings,
    /// 可选 Public realtime 区块；省略时使用稳定 consumer 和有界 relay 默认值。
    #[serde(default)]
    realtime: RealtimeConfig,
    /// 必填的敏感 TOML 相对或绝对路径。
    secrets_file: PathBuf,
    /// 可选的 Admin runtime 非敏感 TOML 路径；仅 Admin 会读取。
    #[serde(default)]
    runtime_config_file: Option<PathBuf>,
    /// 可选缓存区块；省略时使用关闭缓存的默认值。
    #[serde(default)]
    cache: CacheConfig,
    /// 可选拍卖策略区块；省略时使用旧版共享配置的默认模板。
    #[serde(default)]
    auction: AuctionConfig,
    /// 可选 Worker 区块；省略时 PostgreSQL 并发为 8、SQLite 并发为 1，轮询为 60 秒。
    #[serde(default)]
    worker: WorkerConfig,
    /// 可选 HTTP 安全区块；省略时使用内置默认值。
    #[serde(default)]
    http_security: HttpSecurityConfig,
    /// 可选会话区块；省略时使用默认过期和并发限制。
    #[serde(default)]
    session: SessionConfig,
}

struct BuildInput {
    config_path: Option<PathBuf>,
    listen_address: Option<String>,
    listener: Option<HttpListenerConfig>,
    log_filter: Option<String>,
    metrics: MetricsConfig,
    tracing: TracingSettings,
    realtime: RealtimeConfig,
    cache: CacheConfig,
    auction: AuctionConfig,
    worker: WorkerConfig,
    http_security: HttpSecurityConfig,
    session: SessionConfig,
    secrets_path: Option<PathBuf>,
    runtime_config_path: Option<PathBuf>,
}

/// 独立敏感 TOML 的稳定字段集合。新增外部凭据时必须显式扩展，禁止任意键值透传。
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SecretFileConfig {
    /// 数据库连接串；数据库进程按需读取，不能写入主配置。
    database_url: Option<String>,
    /// Redis 连接串；仅启用 Redis 缓存时需要。
    redis_url: Option<String>,
    /// 会话签名密钥；启用会话写入能力的角色需要。
    session_signing_key: Option<String>,
    /// E-Hentai Cookie；启用对应外部集成时需要。
    ehentai_cookie: Option<String>,
    /// MoogleMail Cookie；启用对应外部集成时需要。
    mooglemail_cookie: Option<String>,
    /// Public OTLP headers；只允许放在敏感 TOML，关闭 tracing 时不得提供。
    public_otlp_headers: Option<String>,
    /// Worker OTLP headers；只允许放在敏感 TOML，关闭 tracing 时不得提供。
    worker_otlp_headers: Option<String>,
    /// Admin OTLP headers；只允许放在敏感 TOML，关闭 tracing 时不得提供。
    admin_otlp_headers: Option<String>,
    /// Admin 登录 actor；Admin 登录能力需要与 `admin_secret` 成对提供。
    admin_actor: Option<String>,
    /// Admin 登录 secret；Admin 登录能力需要与 `admin_actor` 成对提供。
    admin_secret: Option<String>,
    /// 装备数据密钥轮换标识；启用装备密钥封装时必须与材料成对提供。
    equipment_data_key_id: Option<String>,
    /// 装备数据密钥材料；只能放敏感文件，必须与标识成对提供。
    equipment_data_key: Option<String>,
    /// 用户登录 key 数据密钥轮换标识；注册/换绑能力需要与材料成对提供。
    user_login_data_key_id: Option<String>,
    /// 用户登录 key 数据密钥材料；只能放敏感文件，不能与装备 key 混用。
    user_login_data_key: Option<String>,
    /// 外部凭据数据密钥轮换标识；A32 凭据密封需要与材料成对提供。
    credential_data_key_id: Option<String>,
    /// 外部凭据数据密钥材料；只能放敏感文件，不能与其他用途数据密钥混用。
    credential_data_key: Option<String>,
}

/// 不会在 `Debug` 输出中暴露内容的敏感字符串。
#[derive(Clone, PartialEq, Eq)]
struct SecretString(Box<str>);

impl SecretString {
    fn new(field: &'static str, value: String) -> Result<Self, ConfigError> {
        if value.trim().is_empty() {
            return Err(ConfigError::EmptySensitiveField { field });
        }

        Ok(Self(value.into_boxed_str()))
    }

    fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PostgresMaintenanceSecretFile {
    /// 只供破坏性运维命令连接同集群 `postgres` 维护库的高权限 URL。
    database_url: String,
}

/// 独立 PostgreSQL 维护敏感 TOML 的脱敏结果。
///
/// 该类型只允许 `auctionctl postgres-rebuild` 在维护窗口短暂使用，不能并入 Public/Admin/Worker
/// 常驻配置，也不会实现序列化或暴露在 `Debug` 输出中。
#[derive(Clone)]
pub struct PostgresMaintenanceSecrets {
    database_url: SecretString,
}

impl PostgresMaintenanceSecrets {
    /// 返回指向同集群 `postgres` 数据库的维护 URL；调用方不得记录或放入进程参数。
    pub fn database_url(&self) -> &str {
        self.database_url.expose_secret()
    }
}

impl fmt::Debug for PostgresMaintenanceSecrets {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PostgresMaintenanceSecrets([REDACTED])")
    }
}

/// `.keyenv` 中仅允许出现的两个 MoogleMail Cookie 身份。
///
/// `hv-ck-key1` 固定解释为 Bot 号，`hv-ck-key2` 固定解释为测试号。该类型不实现序列化，
/// getter 只应在装配 MoogleMail 适配器时短暂调用。
#[derive(Clone)]
pub struct MoogleMailKeyEnv {
    bot_cookie: SecretString,
    test_cookie: SecretString,
}

impl MoogleMailKeyEnv {
    /// 返回 Bot 号 Cookie；调用方不得记录、持久化或传给浏览器。
    pub fn bot_cookie(&self) -> &str {
        self.bot_cookie.expose_secret()
    }

    /// 返回测试号 Cookie；调用方不得记录、持久化或传给浏览器。
    pub fn test_cookie(&self) -> &str {
        self.test_cookie.expose_secret()
    }
}

impl fmt::Debug for MoogleMailKeyEnv {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MoogleMailKeyEnv([REDACTED; 2])")
    }
}

#[derive(Clone, Default)]
struct AppSecrets {
    database_url: Option<SecretString>,
    redis_url: Option<SecretString>,
    session_signing_key: Option<SecretString>,
    ehentai_cookie: Option<SecretString>,
    mooglemail_cookie: Option<SecretString>,
    public_otlp_headers: Option<SecretString>,
    worker_otlp_headers: Option<SecretString>,
    admin_otlp_headers: Option<SecretString>,
    admin_actor: Option<SecretString>,
    admin_secret: Option<SecretString>,
    equipment_data_key_id: Option<SecretString>,
    equipment_data_key: Option<SecretString>,
    user_login_data_key_id: Option<SecretString>,
    user_login_data_key: Option<SecretString>,
    credential_data_key_id: Option<SecretString>,
    credential_data_key: Option<SecretString>,
}

impl AppSecrets {
    fn from_file(secret: SecretFileConfig) -> Result<Self, ConfigError> {
        Ok(Self {
            database_url: secret
                .database_url
                .map(|value| SecretString::new("database_url", value))
                .transpose()?,
            redis_url: secret
                .redis_url
                .map(|value| SecretString::new("redis_url", value))
                .transpose()?,
            session_signing_key: secret
                .session_signing_key
                .map(|value| SecretString::new("session_signing_key", value))
                .transpose()?,
            ehentai_cookie: secret
                .ehentai_cookie
                .map(|value| SecretString::new("ehentai_cookie", value))
                .transpose()?,
            mooglemail_cookie: secret
                .mooglemail_cookie
                .map(|value| SecretString::new("mooglemail_cookie", value))
                .transpose()?,
            public_otlp_headers: secret
                .public_otlp_headers
                .map(|value| SecretString::new("public_otlp_headers", value))
                .transpose()?,
            worker_otlp_headers: secret
                .worker_otlp_headers
                .map(|value| SecretString::new("worker_otlp_headers", value))
                .transpose()?,
            admin_otlp_headers: secret
                .admin_otlp_headers
                .map(|value| SecretString::new("admin_otlp_headers", value))
                .transpose()?,
            admin_actor: secret
                .admin_actor
                .map(|value| SecretString::new("admin_actor", value))
                .transpose()?,
            admin_secret: secret
                .admin_secret
                .map(|value| SecretString::new("admin_secret", value))
                .transpose()?,
            equipment_data_key_id: secret
                .equipment_data_key_id
                .map(|value| SecretString::new("equipment_data_key_id", value))
                .transpose()?,
            equipment_data_key: secret
                .equipment_data_key
                .map(|value| SecretString::new("equipment_data_key", value))
                .transpose()?,
            user_login_data_key_id: secret
                .user_login_data_key_id
                .map(|value| SecretString::new("user_login_data_key_id", value))
                .transpose()?,
            user_login_data_key: secret
                .user_login_data_key
                .map(|value| SecretString::new("user_login_data_key", value))
                .transpose()?,
            credential_data_key_id: secret
                .credential_data_key_id
                .map(|value| SecretString::new("credential_data_key_id", value))
                .transpose()?,
            credential_data_key: secret
                .credential_data_key
                .map(|value| SecretString::new("credential_data_key", value))
                .transpose()?,
        })
    }
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
    secrets: AppSecrets,
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

impl AppConfig {
    pub fn load(role: Role) -> Result<Self, ConfigError> {
        if let Some(path) = env::var_os(CONFIG_FILE_ENV).map(PathBuf::from) {
            return Self::load_from_path(role, path);
        }

        let default_path = PathBuf::from("ops/config").join(format!("{}.toml", role.name()));
        if default_path.is_file() {
            return Self::load_from_path(role, default_path);
        }

        Self::build(
            role,
            BuildInput {
                config_path: None,
                listen_address: None,
                listener: None,
                log_filter: None,
                metrics: MetricsConfig::default(),
                tracing: TracingSettings::default(),
                realtime: RealtimeConfig::default(),
                cache: CacheConfig::default(),
                auction: AuctionConfig::default(),
                worker: WorkerConfig::default(),
                http_security: HttpSecurityConfig::default(),
                session: SessionConfig::default(),
                secrets_path: None,
                runtime_config_path: None,
            },
        )
    }

    pub fn load_from_path(role: Role, path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref().to_owned();
        let contents = fs::read_to_string(&path)
            .map_err(|source| ConfigError::Read { path: path.clone(), source })?;
        let file =
            toml::from_str(&contents).map_err(|_| ConfigError::Parse { path: path.clone() })?;

        Self::from_file(role, Some(path), file)
    }

    /// 返回数据库 URL。调用方不得把返回值写入日志、错误消息或 DTO。
    pub fn database_url(&self) -> Option<&str> {
        self.secrets.database_url.as_ref().map(SecretString::expose_secret)
    }

    /// 返回当前数据库档位下的有效 Worker 并发；SQLite Lite 的省略值固定为 1。
    pub fn worker_concurrency(&self) -> u16 {
        self.worker.concurrency.unwrap_or_else(|| {
            if self.database_url().is_some_and(database_url_is_sqlite) { 1 } else { 8 }
        })
    }

    /// 返回有效的 Worker 轮询间隔秒数；省略时沿用旧版 60 秒。
    pub fn worker_poll_interval_seconds(&self) -> u64 {
        self.worker.poll_interval_seconds.unwrap_or(60)
    }

    /// 返回 Redis URL。只有 `cache.backend = "redis"` 时才能使用。
    pub fn redis_url(&self) -> Option<&str> {
        self.secrets.redis_url.as_ref().map(SecretString::expose_secret)
    }

    /// 返回会话签名密钥。调用方不得输出、持久化副本或传给浏览器。
    pub fn session_signing_key(&self) -> Option<&str> {
        self.secrets.session_signing_key.as_ref().map(SecretString::expose_secret)
    }

    /// 返回 E-Hentai Cookie。只允许外部集成适配器读取。
    pub fn ehentai_cookie(&self) -> Option<&str> {
        self.secrets.ehentai_cookie.as_ref().map(SecretString::expose_secret)
    }

    /// 返回 MoogleMail Cookie。只允许外部集成适配器读取。
    pub fn mooglemail_cookie(&self) -> Option<&str> {
        self.secrets.mooglemail_cookie.as_ref().map(SecretString::expose_secret)
    }

    /// 返回当前角色的 OTLP headers；调用方只允许把它交给 tracing exporter，禁止记录或持久化。
    pub fn tracing_headers(&self) -> Option<&str> {
        match self.role {
            Role::Public => self.secrets.public_otlp_headers.as_ref(),
            Role::Admin => self.secrets.admin_otlp_headers.as_ref(),
            Role::Worker => self.secrets.worker_otlp_headers.as_ref(),
        }
        .map(SecretString::expose_secret)
    }

    /// 返回 Admin 登录 actor；只允许 Admin 装配根读取。
    pub fn admin_actor(&self) -> Option<&str> {
        self.secrets.admin_actor.as_ref().map(SecretString::expose_secret)
    }

    /// 返回 Admin 登录 secret；调用方不得输出、持久化副本或传给浏览器。
    pub fn admin_secret(&self) -> Option<&str> {
        self.secrets.admin_secret.as_ref().map(SecretString::expose_secret)
    }

    /// 返回装备 key 加密使用的数据密钥标识；只允许密钥装配根读取。
    pub fn equipment_data_key_id(&self) -> Option<&str> {
        self.secrets.equipment_data_key_id.as_ref().map(SecretString::expose_secret)
    }

    /// 返回装备 key 加密使用的数据密钥材料；调用方不得输出或持久化副本。
    pub fn equipment_data_key(&self) -> Option<&str> {
        self.secrets.equipment_data_key.as_ref().map(SecretString::expose_secret)
    }

    /// 判断装备 key 加密配置是否成对提供。
    pub fn equipment_data_key_configured(&self) -> bool {
        self.equipment_data_key_id().is_some() && self.equipment_data_key().is_some()
    }

    /// 返回用户登录 key 加密使用的数据密钥标识；只允许密钥装配根读取。
    pub fn user_login_data_key_id(&self) -> Option<&str> {
        self.secrets.user_login_data_key_id.as_ref().map(SecretString::expose_secret)
    }

    /// 返回用户登录 key 加密使用的数据密钥材料；调用方不得输出或持久化副本。
    pub fn user_login_data_key(&self) -> Option<&str> {
        self.secrets.user_login_data_key.as_ref().map(SecretString::expose_secret)
    }

    /// 判断用户登录 key 加密配置是否成对提供。
    pub fn user_login_data_key_configured(&self) -> bool {
        self.user_login_data_key_id().is_some() && self.user_login_data_key().is_some()
    }

    /// 返回外部凭据加密使用的数据密钥标识；只允许凭据装配根读取。
    pub fn credential_data_key_id(&self) -> Option<&str> {
        self.secrets.credential_data_key_id.as_ref().map(SecretString::expose_secret)
    }

    /// 返回外部凭据加密使用的数据密钥材料；调用方不得输出或持久化副本。
    pub fn credential_data_key(&self) -> Option<&str> {
        self.secrets.credential_data_key.as_ref().map(SecretString::expose_secret)
    }

    /// 判断外部凭据加密配置是否成对提供。
    pub fn credential_data_key_configured(&self) -> bool {
        self.credential_data_key_id().is_some() && self.credential_data_key().is_some()
    }

    /// 判断 Admin 登录凭据是否成对配置，不暴露任何凭据内容。
    pub fn admin_credentials_configured(&self) -> bool {
        self.admin_actor().is_some() && self.admin_secret().is_some()
    }

    /// 返回 Admin dashboard 连接 Worker control 使用的 socket 路径。
    ///
    /// runtime 文档省略时使用应用模型的默认路径；该 getter 不读取环境变量，也不暴露敏感值。
    pub fn worker_control_socket_path(&self) -> &str {
        self.admin_runtime
            .as_ref()
            .map(|document| document.worker_control.socket_path.as_str())
            .unwrap_or("/run/hv-auction/worker/control.sock")
    }

    /// 返回 Admin runtime 选择的 Unix listener socket；TCP 模式和其它角色返回 `None`。
    ///
    /// 该 getter 只暴露非敏感监听路径，供部署预检和 readiness 探针与 Admin 装配根使用同一
    /// 权威配置，避免部署控制器仍探测已经被 runtime 覆盖掉的 TCP 地址。
    pub fn admin_unix_listener_socket_path(&self) -> Option<&str> {
        self.admin_runtime.as_ref().and_then(|document| {
            (document.listener.mode == auction_application::AdminRuntimeListenMode::Unix)
                .then_some(document.listener.socket_path.as_str())
        })
    }

    fn from_file(
        role: Role,
        config_path: Option<PathBuf>,
        file: FileConfig,
    ) -> Result<Self, ConfigError> {
        let FileConfig {
            listen_address,
            listener,
            log_filter,
            metrics,
            tracing,
            realtime,
            secrets_file,
            runtime_config_file,
            cache,
            auction,
            worker,
            http_security,
            session,
        } = file;
        let worker = resolve_worker_paths(config_path.as_deref(), worker);
        let secrets_path = Some(resolve_secrets_path(config_path.as_deref(), secrets_file));
        let runtime_config_path =
            runtime_config_file.map(|path| resolve_config_path(config_path.as_deref(), path));

        Self::build(
            role,
            BuildInput {
                config_path,
                listen_address,
                listener,
                log_filter,
                metrics,
                tracing,
                realtime,
                cache,
                auction,
                worker,
                http_security,
                session,
                secrets_path,
                runtime_config_path,
            },
        )
    }

    fn build(role: Role, input: BuildInput) -> Result<Self, ConfigError> {
        let BuildInput {
            config_path,
            listen_address,
            listener,
            log_filter,
            metrics,
            tracing,
            realtime,
            cache,
            auction,
            worker,
            http_security,
            session,
            secrets_path,
            runtime_config_path,
        } = input;
        let secret_file =
            secrets_path.as_deref().map(load_secret_file).transpose()?.unwrap_or_default();
        let secrets = AppSecrets::from_file(secret_file)?;

        let admin_runtime = if role == Role::Admin {
            runtime_config_path.as_deref().map(load_admin_runtime_file).transpose()?
        } else {
            None
        };
        let listener = resolve_http_listener(role, listener, listen_address)?;

        // Admin runtime 文件是 Admin 运行时策略的唯一生效源；否则 metrics 区块虽然能被
        // 解析并通过 runtime API 返回，进程装配却仍会读取主配置，形成“可写但不生效”的假配置。
        let metrics = admin_runtime
            .as_ref()
            .map(|runtime| MetricsConfig { enabled: runtime.metrics.enabled })
            .unwrap_or(metrics);
        let tracing = admin_runtime
            .as_ref()
            .map(|runtime| TracingSettings {
                batch_timeout_seconds: runtime.tracing.batch_timeout_seconds,
                enabled: runtime.tracing.enabled,
                endpoint: runtime.tracing.endpoint.clone(),
                export_timeout_seconds: runtime.tracing.export_timeout_seconds,
                max_export_batch_size: runtime.tracing.max_export_batch_size,
                queue_capacity: runtime.tracing.queue_capacity,
                sample_ratio_ppm: runtime.tracing.sample_ratio_ppm,
                shutdown_timeout_seconds: runtime.tracing.shutdown_timeout_seconds,
            })
            .unwrap_or(tracing);

        if secrets.equipment_data_key_id.is_some() != secrets.equipment_data_key.is_some() {
            return Err(ConfigError::InvalidEquipmentDataKeyConfiguration);
        }
        if secrets.user_login_data_key_id.is_some() != secrets.user_login_data_key.is_some() {
            return Err(ConfigError::InvalidUserLoginDataKeyConfiguration);
        }
        if secrets.credential_data_key_id.is_some() != secrets.credential_data_key.is_some() {
            return Err(ConfigError::InvalidCredentialDataKeyConfiguration);
        }

        if cache.backend == CacheBackend::Redis && secrets.redis_url.is_none() {
            return Err(ConfigError::MissingRedisUrl);
        }
        validate_cache(&cache)?;
        let database_is_sqlite = secrets
            .database_url
            .as_ref()
            .is_some_and(|url| database_url_is_sqlite(url.expose_secret()));
        let worker_concurrency =
            worker.concurrency.unwrap_or(if database_is_sqlite { 1 } else { 8 });
        let worker_poll_interval_seconds = worker.poll_interval_seconds.unwrap_or(60);
        if worker.recovery_retry_delay_seconds == 0 || worker.recovery_retry_delay_seconds > 86_400
        {
            return Err(ConfigError::InvalidWorkerRecoveryRetryDelay);
        }
        validate_auction_worker(&worker, worker_concurrency, worker_poll_interval_seconds)?;
        validate_delivery_worker(&worker.delivery)?;
        validate_fulfillment_worker(&worker.fulfillment)?;
        validate_recovery_worker(&worker.recovery)?;
        validate_auction(&auction)?;
        validate_worker_control(&worker.control)?;
        validate_mooglemail_worker(&worker.mooglemail)?;
        validate_forum_worker(&worker.forum)?;
        validate_operations_alert_worker(&worker.operations_alert)?;
        validate_realtime(&realtime)?;
        let tracing_headers = match role {
            Role::Public => secrets.public_otlp_headers.as_ref(),
            Role::Admin => secrets.admin_otlp_headers.as_ref(),
            Role::Worker => secrets.worker_otlp_headers.as_ref(),
        };
        validate_tracing(&tracing, tracing_headers.map(SecretString::expose_secret))?;
        if role == Role::Worker
            && worker.mooglemail.enabled
            && worker.mooglemail.keyenv_file.is_none()
            && secrets.mooglemail_cookie.is_none()
            && secrets.credential_data_key_id.is_none()
        {
            return Err(ConfigError::MissingMoogleMailCredentials);
        }
        if role == Role::Worker
            && worker.mooglemail.enabled
            && (secrets.user_login_data_key_id.is_none() || secrets.user_login_data_key.is_none())
        {
            return Err(ConfigError::MissingMoogleMailUserLoginDataKey);
        }
        if role == Role::Worker
            && worker.forum.enabled
            && secrets.ehentai_cookie.is_none()
            && secrets.credential_data_key_id.is_none()
        {
            return Err(ConfigError::MissingEhentaiCredentials);
        }
        // 即使 Admin runtime 随后覆盖该区块，也拒绝主配置中无法解析的备用安全策略。
        validate_http_security(&http_security)?;
        validate_session(&session)?;
        if role == Role::Worker && database_is_sqlite && worker_concurrency != 1 {
            return Err(ConfigError::LiteWorkerConcurrency { concurrency: worker_concurrency });
        }

        let admin_uses_unix = role == Role::Admin
            && admin_runtime.as_ref().is_some_and(|runtime| {
                runtime.listener.mode == auction_application::AdminRuntimeListenMode::Unix
            });
        let runtime_listen_address = admin_runtime
            .as_ref()
            .filter(|runtime| {
                runtime.listener.mode == auction_application::AdminRuntimeListenMode::Tcp
            })
            .map(AdminRuntimeDocument::tcp_address)
            .transpose()
            .map_err(|error| ConfigError::InvalidRuntimeConfig {
                path: runtime_config_path.clone().unwrap_or_default(),
                reason: error.to_string(),
            })?
            .map(|address| address.to_string());
        let listen_value = if admin_uses_unix {
            None
        } else {
            runtime_listen_address.or_else(|| {
                listener
                    .as_ref()
                    .filter(|listener| listener.mode == HttpListenMode::Tcp)
                    .map(|listener| listener.address.clone())
            })
        };
        let http_security =
            admin_runtime.as_ref().map(runtime_http_security).unwrap_or(http_security);
        validate_http_security(&http_security)?;
        let public_uses_unix = role == Role::Public
            && listener.as_ref().is_some_and(|listener| listener.mode == HttpListenMode::Unix);
        if http_security.trusted_unix_proxy && !admin_uses_unix && !public_uses_unix {
            return Err(ConfigError::InvalidHttpSecurity(
                "trusted_unix_proxy 只能由 Public 或 Admin Unix listener 启用",
            ));
        }
        let session = admin_runtime.as_ref().map(runtime_session).unwrap_or(session);
        let listen_addr = listen_value
            .map(|value| {
                value.parse().map_err(|source| ConfigError::InvalidAddress {
                    field: "listen_address",
                    value,
                    source,
                })
            })
            .transpose()?;

        Ok(Self {
            role,
            listen_addr,
            listener,
            log_filter,
            metrics,
            tracing,
            realtime,
            cache,
            auction,
            worker,
            http_security,
            session,
            config_path,
            secrets_path,
            runtime_config_path,
            admin_runtime,
            secrets,
        })
    }
}

fn validate_auction(config: &AuctionConfig) -> Result<(), ConfigError> {
    if !matches!(config.settlement.mode.as_str(), "legacy_zero_v0" | "hold_curve_v1") {
        return Err(ConfigError::InvalidAuctionConfig(
            "settlement.mode 只能是 legacy_zero_v0 或 hold_curve_v1",
        ));
    }
    if config.settlement.recovery.mode != "remind_v1" {
        return Err(ConfigError::InvalidAuctionConfig("settlement.recovery.mode 只能是 remind_v1"));
    }
    config
        .policy()
        .validate()
        .map_err(|_| ConfigError::InvalidAuctionConfig("经济、结算或恢复字段边界无效"))
}

fn resolve_http_listener(
    role: Role,
    listener: Option<HttpListenerConfig>,
    legacy_address: Option<String>,
) -> Result<Option<HttpListenerConfig>, ConfigError> {
    if listener.is_some() && legacy_address.is_some() {
        return Err(ConfigError::InvalidListenerConfig(
            "listener 与旧字段 listen_address 不能同时提供",
        ));
    }

    let mut listener = listener
        .or_else(|| legacy_address.map(HttpListenerConfig::tcp))
        .or_else(|| role.default_listen().map(HttpListenerConfig::tcp));
    if let Some(value) = listener.as_mut()
        && value.mode == HttpListenMode::Tcp
        && value.address.is_empty()
    {
        value.address = role.default_listen().unwrap_or_default().to_owned();
    }

    if role == Role::Worker {
        if listener.is_some() {
            return Err(ConfigError::InvalidListenerConfig("Worker 不允许配置 HTTP listener"));
        }
        return Ok(None);
    }

    let listener = listener
        .ok_or(ConfigError::InvalidListenerConfig("Public/Admin 必须提供有效的 HTTP listener"))?;
    validate_http_listener(&listener)?;
    Ok(Some(listener))
}

fn validate_http_listener(config: &HttpListenerConfig) -> Result<(), ConfigError> {
    match config.mode {
        HttpListenMode::Tcp => {
            if !config.group.is_empty()
                || !config.runtime_directory.is_empty()
                || !config.socket_path.is_empty()
                || config.address.parse::<SocketAddr>().is_err()
            {
                return Err(ConfigError::InvalidListenerConfig(
                    "TCP listener 必须只提供有效的 address",
                ));
            }
        }
        HttpListenMode::Unix => {
            if !config.address.is_empty()
                || !valid_listener_group(&config.group)
                || !valid_listener_path(&config.runtime_directory)
                || !valid_listener_path(&config.socket_path)
                || Path::new(&config.socket_path).parent()
                    != Some(Path::new(&config.runtime_directory))
            {
                return Err(ConfigError::InvalidListenerConfig(
                    "Unix listener 必须提供规范化的 runtime_directory、socket_path 和 group",
                ));
            }
        }
    }
    Ok(())
}

fn valid_listener_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.starts_with('/')
        && value != "/"
        && value == value.trim()
        && !value.contains("//")
        && !value.chars().any(char::is_control)
        && !value.split('/').any(|segment| segment == "." || segment == "..")
}

fn valid_listener_group(value: &str) -> bool {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    value.len() <= 64
        && first.is_ascii_alphanumeric()
        && characters
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
}

fn validate_cache(config: &CacheConfig) -> Result<(), ConfigError> {
    if !(1..=3600).contains(&config.public_rules_ttl_seconds) {
        return Err(ConfigError::InvalidCacheConfig("public_rules_ttl_seconds 必须位于 1 至 3600"));
    }
    Ok(())
}

fn validate_http_security(config: &HttpSecurityConfig) -> Result<(), ConfigError> {
    if config.login_rate_limit == 0 || config.login_rate_limit > 10_000 {
        return Err(ConfigError::InvalidHttpSecurity("login_rate_limit 必须位于 1 至 10000"));
    }
    if config.mutation_rate_limit == 0 || config.mutation_rate_limit > 10_000 {
        return Err(ConfigError::InvalidHttpSecurity("mutation_rate_limit 必须位于 1 至 10000"));
    }
    if !(1..=3600).contains(&config.rate_limit_window_seconds) {
        return Err(ConfigError::InvalidHttpSecurity(
            "rate_limit_window_seconds 必须位于 1 至 3600",
        ));
    }
    if !(1..=120).contains(&config.request_timeout_seconds) {
        return Err(ConfigError::InvalidHttpSecurity("request_timeout_seconds 必须位于 1 至 120"));
    }
    if config.trusted_proxy_cidrs.len() > 64 {
        return Err(ConfigError::InvalidHttpSecurity("trusted_proxy_cidrs 最多包含 64 个网段"));
    }
    for (index, value) in config.trusted_proxy_cidrs.iter().enumerate() {
        let network = value
            .parse::<IpNet>()
            .map_err(|_| ConfigError::InvalidHttpSecurity("trusted_proxy_cidrs 必须是规范 CIDR"))?;
        if network.trunc().to_string() != *value
            || config.trusted_proxy_cidrs[..index].iter().any(|existing| existing == value)
        {
            return Err(ConfigError::InvalidHttpSecurity("trusted_proxy_cidrs 必须规范且不能重复"));
        }
    }
    if config.allowed_origins_enabled {
        if config.allowed_origins.is_empty() || config.allowed_origins.len() > 16 {
            return Err(ConfigError::InvalidHttpSecurity(
                "allowed_origins 启用时必须包含 1 至 16 个 origin",
            ));
        }
        for origin in &config.allowed_origins {
            let parsed = Url::parse(origin).map_err(|_| {
                ConfigError::InvalidHttpSecurity("allowed_origins 必须是有效的 HTTP(S) origin")
            })?;
            if !matches!(parsed.scheme(), "http" | "https")
                || parsed.host_str().is_none()
                || parsed.username() != ""
                || parsed.password().is_some()
                || parsed.path() != ""
                || parsed.query().is_some()
                || parsed.fragment().is_some()
                || parsed.as_str() != origin
            {
                return Err(ConfigError::InvalidHttpSecurity(
                    "allowed_origins 只能包含精确 scheme+host",
                ));
            }
        }
    }
    Ok(())
}

fn validate_realtime(config: &RealtimeConfig) -> Result<(), ConfigError> {
    if config.consumer_key.trim().is_empty()
        || config.consumer_key != config.consumer_key.trim()
        || config.consumer_key.len() > 128
        || config.consumer_key.chars().any(char::is_control)
    {
        return Err(ConfigError::InvalidRealtimeConfig(
            "consumer_key 必须是 1 至 128 字节的无控制字符标识",
        ));
    }
    if config.poll_interval_millis == 0 || config.poll_interval_millis > 60_000 {
        return Err(ConfigError::InvalidRealtimeConfig("poll_interval_millis 必须位于 1 至 60000"));
    }
    if config.stream_batch_size == 0 || config.stream_batch_size > 200 {
        return Err(ConfigError::InvalidRealtimeConfig("stream_batch_size 必须位于 1 至 200"));
    }
    if config.event_batch_size == 0 || config.event_batch_size > 200 {
        return Err(ConfigError::InvalidRealtimeConfig("event_batch_size 必须位于 1 至 200"));
    }
    if config.client_buffer == 0 || config.client_buffer > 1024 {
        return Err(ConfigError::InvalidRealtimeConfig("client_buffer 必须位于 1 至 1024"));
    }
    Ok(())
}

fn validate_tracing(config: &TracingSettings, headers: Option<&str>) -> Result<(), ConfigError> {
    if !(1..=30).contains(&config.batch_timeout_seconds)
        || !(1..=30).contains(&config.export_timeout_seconds)
        || !(1..=30).contains(&config.shutdown_timeout_seconds)
        || !(1..=512).contains(&config.max_export_batch_size)
        || !(1..=4096).contains(&config.queue_capacity)
        || config.max_export_batch_size > config.queue_capacity
        || config.enabled && (config.sample_ratio_ppm == 0 || config.sample_ratio_ppm > 1_000_000)
        || !config.enabled && config.sample_ratio_ppm > 1_000_000
    {
        return Err(ConfigError::InvalidTracingConfig("tracing 数值边界无效"));
    }

    let raw_headers = headers.unwrap_or_default();
    if !config.enabled && !raw_headers.is_empty() {
        return Err(ConfigError::InvalidTracingConfig("关闭 tracing 时不能提供 OTLP headers"));
    }
    if config.enabled && config.endpoint.is_empty() {
        return Err(ConfigError::InvalidTracingConfig("启用 tracing 时 endpoint 不能为空"));
    }
    if !config.endpoint.is_empty() {
        let parsed = Url::parse(&config.endpoint)
            .map_err(|_| ConfigError::InvalidTracingConfig("endpoint 必须是绝对 HTTP(S) 地址"))?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || parsed.username() != ""
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.as_str() != config.endpoint
            || config.endpoint.chars().any(char::is_control)
        {
            return Err(ConfigError::InvalidTracingConfig(
                "endpoint 不能包含 userinfo、query、fragment 或控制字符",
            ));
        }
    }
    validate_tracing_headers(raw_headers)
}

fn validate_tracing_headers(value: &str) -> Result<(), ConfigError> {
    if value.is_empty() {
        return Ok(());
    }
    if value.len() > 8 * 1024 || value.chars().any(char::is_control) {
        return Err(ConfigError::InvalidTracingConfig(
            "OTLP headers 必须是不超过 8192 字节的安全列表",
        ));
    }
    let parts = value.split(',').collect::<Vec<_>>();
    if parts.len() > 32 {
        return Err(ConfigError::InvalidTracingConfig("OTLP headers 最多包含 32 项"));
    }
    let mut seen = BTreeSet::new();
    for part in parts {
        let Some((key, header_value)) = part.split_once('=') else {
            return Err(ConfigError::InvalidTracingConfig("OTLP headers 必须使用 key=value 格式"));
        };
        let key = key.trim();
        let header_value = header_value.trim();
        if !valid_http_header_token(key)
            || header_value.is_empty()
            || header_value.len() > 1024
            || header_value.chars().any(char::is_control)
        {
            return Err(ConfigError::InvalidTracingConfig("OTLP headers 包含非法或过长的键值"));
        }
        let canonical = key.to_ascii_lowercase();
        if matches!(
            canonical.as_str(),
            "baggage"
                | "connection"
                | "content-encoding"
                | "content-length"
                | "content-type"
                | "host"
                | "traceparent"
                | "tracestate"
                | "transfer-encoding"
        ) || !seen.insert(canonical)
        {
            return Err(ConfigError::InvalidTracingConfig("OTLP headers 含有禁止覆盖或重复的键"));
        }
    }
    Ok(())
}

fn valid_http_header_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn validate_session(config: &SessionConfig) -> Result<(), ConfigError> {
    if !(60..=24 * 60 * 60).contains(&config.idle_ttl_seconds) {
        return Err(ConfigError::InvalidSessionConfig("idle_ttl_seconds 必须位于 60 至 86400"));
    }
    if config.absolute_ttl_seconds < config.idle_ttl_seconds
        || config.absolute_ttl_seconds > 30 * 24 * 60 * 60
    {
        return Err(ConfigError::InvalidSessionConfig(
            "absolute_ttl_seconds 必须不小于 idle_ttl_seconds 且不大于 2592000",
        ));
    }
    if config.max_public_sessions == 0 || config.max_public_sessions > 100 {
        return Err(ConfigError::InvalidSessionConfig("max_public_sessions 必须位于 1 至 100"));
    }
    Ok(())
}

fn resolve_secrets_path(config_path: Option<&Path>, requested_path: PathBuf) -> PathBuf {
    if requested_path.is_absolute() {
        return requested_path;
    }

    config_path.and_then(Path::parent).unwrap_or_else(|| Path::new(".")).join(requested_path)
}

fn resolve_config_path(config_path: Option<&Path>, requested_path: PathBuf) -> PathBuf {
    if requested_path.is_absolute() {
        return requested_path;
    }

    config_path.and_then(Path::parent).unwrap_or_else(|| Path::new(".")).join(requested_path)
}

fn resolve_worker_paths(config_path: Option<&Path>, mut worker: WorkerConfig) -> WorkerConfig {
    worker.mooglemail.keyenv_file =
        worker.mooglemail.keyenv_file.map(|path| resolve_config_path(config_path, path));
    worker
}

fn database_url_is_sqlite(value: &str) -> bool {
    value.split_once(':').is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("sqlite"))
}

fn validate_auction_worker(
    config: &WorkerConfig,
    concurrency: u16,
    poll_interval_seconds: u64,
) -> Result<(), ConfigError> {
    if config.batch_limit == 0 || config.batch_limit > 1000 {
        return Err(ConfigError::InvalidAuctionWorkerConfig("batch_limit 必须位于 1 至 1000"));
    }
    if concurrency == 0 || concurrency > config.batch_limit {
        return Err(ConfigError::InvalidAuctionWorkerConfig(
            "concurrency 必须位于 1 至 batch_limit",
        ));
    }
    if poll_interval_seconds == 0 || poll_interval_seconds > 86_400 {
        return Err(ConfigError::InvalidAuctionWorkerConfig(
            "poll_interval_seconds 必须位于 1 至 86400",
        ));
    }
    if config.jitter_seconds > 86_400 {
        return Err(ConfigError::InvalidAuctionWorkerConfig("jitter_seconds 必须位于 0 至 86400"));
    }
    let lease_duration_seconds = poll_interval_seconds.saturating_mul(3).max(30);
    if config.item_timeout_seconds == 0
        || config.item_timeout_seconds >= lease_duration_seconds
        || config.item_timeout_seconds > 86_400
    {
        return Err(ConfigError::InvalidAuctionWorkerConfig(
            "item_timeout_seconds 必须大于 0、小于派生 claim lease 且不超过 86400",
        ));
    }
    Ok(())
}

fn validate_delivery_worker(config: &DeliveryWorkerConfig) -> Result<(), ConfigError> {
    if config.batch_limit == 0 || config.batch_limit > 1000 {
        return Err(ConfigError::InvalidDeliveryWorkerConfig("batch_limit 必须位于 1 至 1000"));
    }
    if config.concurrency == 0 || config.concurrency > config.batch_limit {
        return Err(ConfigError::InvalidDeliveryWorkerConfig(
            "concurrency 必须位于 1 至 batch_limit",
        ));
    }
    if config.poll_interval_seconds == 0 || config.poll_interval_seconds > 86_400 {
        return Err(ConfigError::InvalidDeliveryWorkerConfig(
            "poll_interval_seconds 必须位于 1 至 86400",
        ));
    }
    if config.jitter_seconds > 86_400 {
        return Err(ConfigError::InvalidDeliveryWorkerConfig("jitter_seconds 必须位于 0 至 86400"));
    }
    if config.lease_duration_seconds == 0 || config.lease_duration_seconds > 86_400 {
        return Err(ConfigError::InvalidDeliveryWorkerConfig(
            "lease_duration_seconds 必须位于 1 至 86400",
        ));
    }
    if config.renew_interval_seconds == 0
        || config.renew_interval_seconds >= config.lease_duration_seconds
    {
        return Err(ConfigError::InvalidDeliveryWorkerConfig(
            "renew_interval_seconds 必须大于 0 且小于 lease_duration_seconds",
        ));
    }
    if config.request_timeout_seconds == 0 || config.request_timeout_seconds > 86_400 {
        return Err(ConfigError::InvalidDeliveryWorkerConfig(
            "request_timeout_seconds 必须位于 1 至 86400",
        ));
    }
    if config.sending_grace_seconds <= config.request_timeout_seconds
        || config.sending_grace_seconds > 86_400
    {
        return Err(ConfigError::InvalidDeliveryWorkerConfig(
            "sending_grace_seconds 必须大于 request_timeout_seconds 且不超过 86400",
        ));
    }
    if config.finalize_timeout_seconds == 0 || config.finalize_timeout_seconds > 300 {
        return Err(ConfigError::InvalidDeliveryWorkerConfig(
            "finalize_timeout_seconds 必须位于 1 至 300",
        ));
    }
    Ok(())
}

fn validate_fulfillment_worker(config: &FulfillmentWorkerConfig) -> Result<(), ConfigError> {
    if config.batch_limit == 0 || config.batch_limit > 1000 {
        return Err(ConfigError::InvalidFulfillmentWorkerConfig("batch_limit 必须位于 1 至 1000"));
    }
    if config.concurrency == 0 || config.concurrency > config.batch_limit {
        return Err(ConfigError::InvalidFulfillmentWorkerConfig(
            "concurrency 必须位于 1 至 batch_limit",
        ));
    }
    if config.poll_interval_seconds == 0 || config.poll_interval_seconds > 86_400 {
        return Err(ConfigError::InvalidFulfillmentWorkerConfig(
            "poll_interval_seconds 必须位于 1 至 86400",
        ));
    }
    if config.jitter_seconds > 86_400 {
        return Err(ConfigError::InvalidFulfillmentWorkerConfig(
            "jitter_seconds 必须位于 0 至 86400",
        ));
    }
    if config.lease_duration_seconds == 0 || config.lease_duration_seconds > 86_400 {
        return Err(ConfigError::InvalidFulfillmentWorkerConfig(
            "lease_duration_seconds 必须位于 1 至 86400",
        ));
    }
    if config.item_timeout_seconds == 0
        || config.item_timeout_seconds >= config.lease_duration_seconds
    {
        return Err(ConfigError::InvalidFulfillmentWorkerConfig(
            "item_timeout_seconds 必须大于 0 且小于 lease_duration_seconds",
        ));
    }
    if config.retry_delay_seconds == 0 || config.retry_delay_seconds > 86_400 {
        return Err(ConfigError::InvalidFulfillmentWorkerConfig(
            "retry_delay_seconds 必须位于 1 至 86400",
        ));
    }
    Ok(())
}

fn validate_recovery_worker(config: &RecoveryWorkerConfig) -> Result<(), ConfigError> {
    if config.batch_limit == 0 || config.batch_limit > 1000 {
        return Err(ConfigError::InvalidRecoveryWorkerConfig("batch_limit 必须位于 1 至 1000"));
    }
    if config.concurrency == 0 || config.concurrency > config.batch_limit {
        return Err(ConfigError::InvalidRecoveryWorkerConfig(
            "concurrency 必须位于 1 至 batch_limit",
        ));
    }
    if config.poll_interval_seconds == 0 || config.poll_interval_seconds > 86_400 {
        return Err(ConfigError::InvalidRecoveryWorkerConfig(
            "poll_interval_seconds 必须位于 1 至 86400",
        ));
    }
    if config.jitter_seconds > 86_400 {
        return Err(ConfigError::InvalidRecoveryWorkerConfig("jitter_seconds 必须位于 0 至 86400"));
    }
    if config.lease_duration_seconds == 0 || config.lease_duration_seconds > 86_400 {
        return Err(ConfigError::InvalidRecoveryWorkerConfig(
            "lease_duration_seconds 必须位于 1 至 86400",
        ));
    }
    if config.item_timeout_seconds == 0
        || config.item_timeout_seconds >= config.lease_duration_seconds
    {
        return Err(ConfigError::InvalidRecoveryWorkerConfig(
            "item_timeout_seconds 必须大于 0 且小于 lease_duration_seconds",
        ));
    }
    if config.low_balance_threshold < 0 {
        return Err(ConfigError::InvalidRecoveryWorkerConfig("low_balance_threshold 不能为负值"));
    }
    if config.health_refresh_interval_seconds == 0
        || config.health_refresh_interval_seconds > 86_400
    {
        return Err(ConfigError::InvalidRecoveryWorkerConfig(
            "health_refresh_interval_seconds 必须位于 1 至 86400",
        ));
    }
    if config.evidence_stale_after_seconds < config.health_refresh_interval_seconds
        || config.evidence_stale_after_seconds > 86_400
    {
        return Err(ConfigError::InvalidRecoveryWorkerConfig(
            "evidence_stale_after_seconds 必须不小于 health_refresh_interval_seconds 且不超过 86400",
        ));
    }
    Ok(())
}

fn validate_worker_control(config: &WorkerControlConfig) -> Result<(), ConfigError> {
    let socket_path = Path::new(&config.socket_path);
    let runtime_directory = Path::new(&config.runtime_directory);
    if !valid_worker_control_path(runtime_directory, None) {
        return Err(ConfigError::InvalidWorkerControlConfig(
            "runtime_directory 必须是规范化的非根绝对路径",
        ));
    }
    if !valid_worker_control_path(socket_path, Some(100)) {
        return Err(ConfigError::InvalidWorkerControlConfig(
            "socket_path 必须是规范化绝对路径且长度不超过 100 字节",
        ));
    }
    if socket_path.parent() != Some(runtime_directory) {
        return Err(ConfigError::InvalidWorkerControlConfig(
            "socket_path 的直接父目录必须等于 runtime_directory",
        ));
    }
    if !valid_safe_identifier(&config.group) {
        return Err(ConfigError::InvalidWorkerControlConfig("group 必须是 1 至 64 位安全标识"));
    }
    if config.allowed_peer_uids.len() + config.allowed_peer_gids.len() > 64 {
        return Err(ConfigError::InvalidWorkerControlConfig("peer allowlist 不能超过 64 项"));
    }
    if duplicate_u32(&config.allowed_peer_uids) || duplicate_u32(&config.allowed_peer_gids) {
        return Err(ConfigError::InvalidWorkerControlConfig("peer allowlist 不能包含重复 ID"));
    }
    if config.enabled && config.allowed_peer_uids.is_empty() && config.allowed_peer_gids.is_empty()
    {
        return Err(ConfigError::InvalidWorkerControlConfig(
            "启用 Worker control 时必须显式配置 peer UID 或 GID allowlist",
        ));
    }
    Ok(())
}

fn valid_worker_control_path(path: &Path, maximum_bytes: Option<usize>) -> bool {
    let normalized: PathBuf = path.components().collect();
    path.is_absolute()
        && path != Path::new("/")
        && normalized.as_os_str() == path.as_os_str()
        && !path.components().any(|component| {
            matches!(component, std::path::Component::CurDir | std::path::Component::ParentDir)
        })
        && !path.as_os_str().to_string_lossy().chars().any(|character| character == '\0')
        && maximum_bytes.is_none_or(|maximum| path.as_os_str().len() <= maximum)
}

fn valid_safe_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(first) if first.is_ascii_alphanumeric())
        && value.len() <= 64
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn duplicate_u32(values: &[u32]) -> bool {
    let mut values = values.to_vec();
    values.sort_unstable();
    values.windows(2).any(|pair| pair[0] == pair[1])
}

fn validate_mooglemail_worker(config: &MoogleMailWorkerConfig) -> Result<(), ConfigError> {
    validate_mooglemail_world(&config.world_id, &config.mailbox_key, &config.sender_allowlist)?;
    if config.start_price.is_some_and(|value| value <= 0) {
        return Err(ConfigError::InvalidMoogleMailConfig("start_price 覆盖值必须大于零"));
    }
    if config.batch_size == 0 || config.batch_size > 1000 {
        return Err(ConfigError::InvalidMoogleMailConfig("batch_size 必须位于 1 至 1000"));
    }
    if config.maximum_pages == 0 || config.maximum_pages > 100 {
        return Err(ConfigError::InvalidMoogleMailConfig("maximum_pages 必须位于 1 至 100"));
    }
    if !valid_mooglemail_endpoint(&config.endpoint) {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "endpoint 必须是固定生产 HTTPS origin 或无附加字段的回环 HTTP(S) origin",
        ));
    }
    if config.maximum_response_bytes == 0 || config.maximum_response_bytes > 8 * 1024 * 1024 {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "maximum_response_bytes 必须位于 1 至 8 MiB",
        ));
    }
    if config.minimum_request_interval_millis == 0
        || config.minimum_request_interval_millis > 60_000
    {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "minimum_request_interval_millis 必须位于 1 至 60000",
        ));
    }
    if !(1..=300).contains(&config.request_timeout_seconds) {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "request_timeout_seconds 必须位于 1 至 300",
        ));
    }
    let schedule = StandardCronSchedule::parse(&config.schedule).map_err(|_| {
        ConfigError::InvalidMoogleMailConfig(
            "schedule 必须是标准五字段 cron、受支持描述符或 @every duration",
        )
    })?;
    if schedule.next_after(Utc::now()).is_none() {
        return Err(ConfigError::InvalidMoogleMailConfig("schedule 必须存在可计算的未来触发时刻"));
    }
    if config.jitter_seconds > 86_400 {
        return Err(ConfigError::InvalidMoogleMailConfig("jitter_seconds 必须位于 0 至 86400"));
    }
    if config.lease_duration_seconds == 0 || config.lease_duration_seconds > 86_400 {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "lease_duration_seconds 必须位于 1 至 86400",
        ));
    }
    if config.renew_interval_seconds == 0
        || config.renew_interval_seconds >= config.lease_duration_seconds
    {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "renew_interval_seconds 必须大于 0 且小于 lease_duration_seconds",
        ));
    }
    if !(1..=10).contains(&config.fetch_attempts) {
        return Err(ConfigError::InvalidMoogleMailConfig("fetch_attempts 必须位于 1 至 10"));
    }
    if !(1..=300).contains(&config.fetch_timeout_seconds)
        || config.fetch_timeout_seconds <= config.request_timeout_seconds
    {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "fetch_timeout_seconds 必须位于 1 至 300 且大于 request_timeout_seconds",
        ));
    }
    if !(1..=10).contains(&config.process_attempts) {
        return Err(ConfigError::InvalidMoogleMailConfig("process_attempts 必须位于 1 至 10"));
    }
    if !(1..=300).contains(&config.item_timeout_seconds)
        || config.item_timeout_seconds <= config.request_timeout_seconds
    {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "item_timeout_seconds 必须位于 1 至 300 且大于 request_timeout_seconds",
        ));
    }
    if config.retry_base_seconds == 0
        || config.retry_base_seconds > config.retry_maximum_seconds
        || config.retry_maximum_seconds > 300
    {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "retry_base_seconds 与 retry_maximum_seconds 必须位于 1 至 300 且 maximum 不小于 base",
        ));
    }
    validate_mooglemail_equipment_filter(&config.equipment_filter)?;
    validate_mooglemail_notification_templates(&config.mail_templates)?;
    let mut worlds = BTreeSet::new();
    worlds.insert(config.world_id.as_str());
    for world in &config.additional_worlds {
        validate_mooglemail_world(&world.world_id, &world.mailbox_key, &world.sender_allowlist)?;
        if !worlds.insert(world.world_id.as_str()) {
            return Err(ConfigError::InvalidMoogleMailConfig(
                "additional_worlds 不得重复配置同一 world_id",
            ));
        }
    }
    let mut mailboxes = BTreeSet::new();
    if !mailboxes.insert(config.mailbox_key.as_str()) {
        return Err(ConfigError::InvalidMoogleMailConfig("mailbox_key 必须唯一"));
    }
    for world in &config.additional_worlds {
        if !mailboxes.insert(world.mailbox_key.as_str()) {
            return Err(ConfigError::InvalidMoogleMailConfig(
                "additional_worlds 的 mailbox_key 必须唯一",
            ));
        }
    }
    Ok(())
}

fn valid_mooglemail_endpoint(value: &str) -> bool {
    if value.is_empty()
        || value != value.trim()
        || value.len() > 2048
        || value.chars().any(char::is_control)
    {
        return false;
    }
    let Ok(endpoint) = Url::parse(value) else {
        return false;
    };
    let path_is_root = endpoint.path().is_empty() || endpoint.path() == "/";
    let common = endpoint.host_str().is_some()
        && endpoint.username().is_empty()
        && endpoint.password().is_none()
        && path_is_root
        && endpoint.query().is_none()
        && endpoint.fragment().is_none();
    if !common {
        return false;
    }
    if endpoint.scheme() == "https"
        && endpoint.host_str().is_some_and(|host| host.eq_ignore_ascii_case("hentaiverse.org"))
        && endpoint.port().is_none()
    {
        return true;
    }
    if !matches!(endpoint.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = endpoint.host_str() else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|address| address.is_loopback())
}

fn validate_mooglemail_equipment_filter(
    config: &MoogleMailEquipmentFilterConfig,
) -> Result<(), ConfigError> {
    if config.level_min < 0 || config.level_max < config.level_min || config.level_max > 1000 {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "equipment_filter 的等级范围必须位于 0 至 1000 且 max 不小于 min",
        ));
    }
    let lists = [
        &config.qualities,
        &config.prefixes,
        &config.suffixes,
        &config.categories,
        &config.equipment_types,
    ];
    let mut filter_count = 0;
    for list in lists {
        if list.len() > 256 {
            return Err(ConfigError::InvalidMoogleMailConfig(
                "equipment_filter 每个 token 列表最多 256 项",
            ));
        }
        filter_count += list.len();
        for (index, value) in list.iter().enumerate() {
            if !valid_config_text(value, 128)
                || list[..index].iter().any(|previous| previous == value)
            {
                return Err(ConfigError::InvalidMoogleMailConfig(
                    "equipment_filter token 必须是唯一、非空且不含控制字符的文本",
                ));
            }
        }
    }
    if config.enabled && filter_count == 0 && config.level_min == 0 && config.level_max == 0 {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "启用 equipment_filter 时至少需要一个过滤条件",
        ));
    }
    Ok(())
}

fn validate_mooglemail_notification_templates(
    config: &MoogleMailNotificationTemplatesConfig,
) -> Result<(), ConfigError> {
    let templates = [
        ("auction_assets_rejected", config.auction_assets_rejected.as_str(), &[][..], &[][..]),
        ("auction_no_attachments", config.auction_no_attachments.as_str(), &[][..], &[][..]),
        (
            "auction_equipment_rejected",
            config.auction_equipment_rejected.as_str(),
            &["reason_code"][..],
            &["reason_code"][..],
        ),
        ("user_assets_rejected", config.user_assets_rejected.as_str(), &[][..], &[][..]),
        (
            "register_key",
            config.register_key.as_str(),
            &["login_key", "username"][..],
            &["login_key", "username"][..],
        ),
        (
            "register_result",
            config.register_result.as_str(),
            &["result_code", "username"][..],
            &["result_code", "username"][..],
        ),
        (
            "rebind_result",
            config.rebind_result.as_str(),
            &["result_code", "username"][..],
            &["result_code", "username"][..],
        ),
        (
            "settlement_winner_equipment_cod",
            config.settlement_winner_equipment_cod.as_str(),
            &["item_name", "amount", "reference"][..],
            &["item_name", "amount", "reference"][..],
        ),
        (
            "settlement_payment_carrier",
            config.settlement_payment_carrier.as_str(),
            &["amount", "reference"][..],
            &["amount", "reference"][..],
        ),
        (
            "settlement_payment_reminder",
            config.settlement_payment_reminder.as_str(),
            &["reference"][..],
            &["reference"][..],
        ),
        (
            "settlement_equipment_handoff",
            config.settlement_equipment_handoff.as_str(),
            &["item_name", "reference"][..],
            &["item_name", "reference"][..],
        ),
        (
            "settlement_seller_payout",
            config.settlement_seller_payout.as_str(),
            &["amount", "reference"][..],
            &["amount", "reference"][..],
        ),
        (
            "settlement_buyer_refund",
            config.settlement_buyer_refund.as_str(),
            &["amount", "reference"][..],
            &["amount", "reference"][..],
        ),
        ("operations_alert", config.operations_alert.as_str(), &["summary"][..], &["summary"][..]),
    ];
    for (_name, value, required, allowed) in templates {
        validate_mooglemail_template_body(value, required, allowed)?;
    }
    Ok(())
}

fn validate_mooglemail_template_body(
    value: &str,
    required: &[&str],
    allowed: &[&str],
) -> Result<(), ConfigError> {
    if value.is_empty()
        || value != value.trim()
        || value.len() > 64 * 1024
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(ConfigError::InvalidMoogleMailConfig("mail_templates 正文格式无效"));
    }
    let mut cursor = 0;
    let mut seen = BTreeSet::new();
    while let Some(relative_start) = value[cursor..].find('{') {
        let start = cursor + relative_start;
        let relative_end = value[start + 1..]
            .find('}')
            .ok_or(ConfigError::InvalidMoogleMailConfig("mail_templates 占位符未闭合"))?;
        let end = start + 1 + relative_end;
        let placeholder = &value[start + 1..end];
        if !allowed.contains(&placeholder) {
            return Err(ConfigError::InvalidMoogleMailConfig("mail_templates 含有未知占位符"));
        }
        seen.insert(placeholder);
        cursor = end + 1;
    }
    if value[cursor..].contains('}')
        || required.iter().any(|placeholder| !seen.contains(placeholder))
    {
        return Err(ConfigError::InvalidMoogleMailConfig("mail_templates 缺少必需占位符"));
    }
    Ok(())
}

fn validate_mooglemail_world(
    world_id: &str,
    mailbox_key: &str,
    sender_allowlist: &[String],
) -> Result<(), ConfigError> {
    if !valid_config_text(mailbox_key, 256) {
        return Err(ConfigError::InvalidMoogleMailConfig("mailbox_key 无效"));
    }
    if !matches!(world_id, "main" | "isekai") {
        return Err(ConfigError::InvalidMoogleMailConfig("world_id 只能是 main 或 isekai"));
    }
    for (index, sender) in sender_allowlist.iter().enumerate() {
        if !valid_config_text(sender, 256)
            || sender_allowlist[..index]
                .iter()
                .any(|previous| previous.eq_ignore_ascii_case(sender))
        {
            return Err(ConfigError::InvalidMoogleMailConfig(
                "sender_allowlist 含有重复或无效发件人",
            ));
        }
    }
    Ok(())
}

fn validate_forum_worker(config: &ForumSyncWorkerConfig) -> Result<(), ConfigError> {
    if !matches!(config.credential_world.as_str(), "main" | "isekai" | "shared") {
        return Err(ConfigError::InvalidForumConfig(
            "credential_world 只能是 main、isekai 或 shared",
        ));
    }
    if config.batch_size == 0 || config.batch_size > 100 {
        return Err(ConfigError::InvalidForumConfig("batch_size 必须位于 1 至 100"));
    }
    if config.maximum_response_bytes == 0 || config.maximum_response_bytes > 8 * 1024 * 1024 {
        return Err(ConfigError::InvalidForumConfig("maximum_response_bytes 必须位于 1 至 8 MiB"));
    }
    if config.minimum_request_interval_millis == 0 {
        return Err(ConfigError::InvalidForumConfig("minimum_request_interval_millis 必须大于零"));
    }
    if !(1..=300).contains(&config.request_timeout_seconds) {
        return Err(ConfigError::InvalidForumConfig("request_timeout_seconds 必须位于 1 至 300"));
    }
    if config.item_timeout_seconds == 0 || config.item_timeout_seconds > 3_600 {
        return Err(ConfigError::InvalidForumConfig("item_timeout_seconds 必须位于 1 至 3600"));
    }
    let request_budget_millis = config
        .request_timeout_seconds
        .saturating_mul(3)
        .saturating_mul(1_000)
        .saturating_add(config.minimum_request_interval_millis.saturating_mul(2))
        .saturating_add(1_000);
    if config.item_timeout_seconds.saturating_mul(1_000) < request_budget_millis {
        return Err(ConfigError::InvalidForumConfig(
            "item_timeout_seconds 必须覆盖三次请求、两次最小请求间隔和一秒收口预算",
        ));
    }
    if config.lease_duration_seconds == 0 || config.lease_duration_seconds > 86_400 {
        return Err(ConfigError::InvalidForumConfig("lease_duration_seconds 必须位于 1 至 86400"));
    }
    if config.item_timeout_seconds >= config.lease_duration_seconds {
        return Err(ConfigError::InvalidForumConfig(
            "item_timeout_seconds 必须小于 lease_duration_seconds",
        ));
    }
    if let Some(endpoint) = &config.endpoint {
        let parsed = Url::parse(endpoint)
            .map_err(|_| ConfigError::InvalidForumConfig("endpoint 不是有效 URL"))?;
        let loopback_http = parsed.scheme() == "http"
            && matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
        if !(parsed.scheme() == "https" || loopback_http)
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(ConfigError::InvalidForumConfig(
                "endpoint 必须是无凭据的 HTTPS origin 或回环 HTTP origin",
            ));
        }
    }
    if let Some(public_base_url) = &config.public_base_url {
        let parsed = Url::parse(public_base_url)
            .map_err(|_| ConfigError::InvalidForumConfig("public_base_url 不是有效 URL"))?;
        if !(parsed.scheme() == "https"
            || (parsed.scheme() == "http"
                && matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1"))))
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.path() != ""
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || public_base_url.trim() != public_base_url
        {
            return Err(ConfigError::InvalidForumConfig(
                "public_base_url 必须是无凭据、无路径和无查询的 HTTPS origin 或回环 HTTP origin",
            ));
        }
    }
    Ok(())
}

fn validate_operations_alert_worker(
    config: &OperationsAlertWorkerConfig,
) -> Result<(), ConfigError> {
    if config.scan_interval_seconds == 0 || config.scan_interval_seconds > 86_400 {
        return Err(ConfigError::InvalidOperationsAlertConfig(
            "scan_interval_seconds 必须位于 1 至 86400",
        ));
    }
    let policy = config.policy();
    if policy.validate().is_err() {
        return Err(ConfigError::InvalidOperationsAlertConfig(
            "时间、阈值、批次数量或正文大小超出允许范围",
        ));
    }
    if !matches!(config.world_id.as_str(), "main" | "isekai") {
        return Err(ConfigError::InvalidOperationsAlertConfig("world_id 只能是 main 或 isekai"));
    }
    if config.enabled && !valid_config_text(&config.recipient, 256) {
        return Err(ConfigError::InvalidOperationsAlertConfig(
            "启用告警时 recipient 必须是非空且不含控制字符的文本",
        ));
    }
    if !config.recipient.is_empty() && !valid_config_text(&config.recipient, 256) {
        return Err(ConfigError::InvalidOperationsAlertConfig("recipient 无效"));
    }
    Ok(())
}

fn valid_config_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn load_secret_file(path: &Path) -> Result<SecretFileConfig, ConfigError> {
    let contents = fs::read_to_string(path)
        .map_err(|source| ConfigError::SecretRead { path: path.to_owned(), source })?;

    toml::from_str(&contents).map_err(|_| ConfigError::SecretParse { path: path.to_owned() })
}

/// 加载破坏性 PostgreSQL 运维专用的独立敏感 TOML。
///
/// 路径必须是规范化绝对路径，文件不得是 symlink，Unix 权限不得向 group/other 开放。解析错误
/// 固定脱敏，确保连接串不会进入诊断。
pub fn load_postgres_maintenance_secrets(
    path: impl AsRef<Path>,
) -> Result<PostgresMaintenanceSecrets, ConfigError> {
    const MAX_MAINTENANCE_SECRET_BYTES: u64 = 64 * 1024;
    let path = path.as_ref().to_owned();
    if !path.is_absolute() || path == Path::new("/") {
        return Err(ConfigError::InvalidMaintenanceSecretPath { path });
    }
    let canonical = fs::canonicalize(&path)
        .map_err(|source| ConfigError::SecretRead { path: path.clone(), source })?;
    if canonical != path {
        return Err(ConfigError::InvalidMaintenanceSecretPath { path });
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| ConfigError::InvalidMaintenanceSecretPath { path: path.clone() })?;
    let parent_metadata = fs::symlink_metadata(parent)
        .map_err(|source| ConfigError::SecretRead { path: parent.to_owned(), source })?;
    if !parent_metadata.file_type().is_dir() {
        return Err(ConfigError::InvalidMaintenanceSecretPath { path });
    }
    #[cfg(unix)]
    if parent_metadata.permissions().mode() & 0o022 != 0 {
        return Err(ConfigError::InsecureMaintenanceSecretParent { path: parent.to_owned() });
    }

    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut file = options
        .open(&path)
        .map_err(|source| ConfigError::SecretRead { path: path.clone(), source })?;
    let metadata =
        file.metadata().map_err(|source| ConfigError::SecretRead { path: path.clone(), source })?;
    if !metadata.file_type().is_file() {
        return Err(ConfigError::InvalidMaintenanceSecretFile { path });
    }
    #[cfg(unix)]
    {
        // 维护 URL 可能具备删库权限；除 mode 外还要求单 hard-link 且由当前有效用户拥有，避免
        // root/共享目录外的另一个账号替换同 inode 后继续复用。
        let effective_uid = unsafe { libc::geteuid() };
        if metadata.permissions().mode() & 0o077 != 0
            || metadata.nlink() != 1
            || metadata.uid() != effective_uid
        {
            return Err(ConfigError::InsecureMaintenanceSecretPermissions { path });
        }
    }
    if metadata.len() == 0 || metadata.len() > MAX_MAINTENANCE_SECRET_BYTES {
        return Err(ConfigError::SecretParse { path });
    }
    let mut contents = String::with_capacity(metadata.len() as usize);
    (&mut file)
        .take(MAX_MAINTENANCE_SECRET_BYTES + 1)
        .read_to_string(&mut contents)
        .map_err(|source| ConfigError::SecretRead { path: path.clone(), source })?;
    if contents.len() as u64 > MAX_MAINTENANCE_SECRET_BYTES {
        return Err(ConfigError::SecretParse { path });
    }
    let secret: PostgresMaintenanceSecretFile =
        toml::from_str(&contents).map_err(|_| ConfigError::SecretParse { path: path.clone() })?;
    Ok(PostgresMaintenanceSecrets {
        database_url: SecretString::new("maintenance_database_url", secret.database_url)?,
    })
}

/// 读取本地 `.keyenv` 的两个 MoogleMail Cookie。
///
/// 这是用户明确提供的本地开发/联调凭据通道，不是通用环境变量覆盖机制。解析器只接受
/// `hv-ck-key1` 和 `hv-ck-key2`，拒绝未知键、重复键、空值和不安全文件权限；错误不会回显
/// 任何值。
pub fn load_mooglemail_keyenv(path: impl AsRef<Path>) -> Result<MoogleMailKeyEnv, ConfigError> {
    let path = path.as_ref().to_owned();
    let metadata = fs::metadata(&path)
        .map_err(|source| ConfigError::MoogleMailKeyEnvRead { path: path.clone(), source })?;
    if !metadata.is_file() {
        return Err(ConfigError::MoogleMailKeyEnvRead {
            path,
            source: io::Error::new(io::ErrorKind::InvalidInput, "path is not a regular file"),
        });
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(ConfigError::MoogleMailKeyEnvInsecurePermissions { path });
    }

    let contents = fs::read_to_string(&path)
        .map_err(|source| ConfigError::MoogleMailKeyEnvRead { path: path.clone(), source })?;
    let mut bot_cookie = None;
    let mut test_cookie = None;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((raw_key, raw_value)) = line.split_once('=') else {
            return Err(ConfigError::MoogleMailKeyEnvParse { path: path.clone() });
        };
        let key = raw_key.trim();
        let Some(value) = parse_mooglemail_keyenv_value(raw_value) else {
            return Err(ConfigError::MoogleMailKeyEnvParse { path: path.clone() });
        };
        match key {
            "hv-ck-key1" if bot_cookie.is_none() => bot_cookie = Some(value),
            "hv-ck-key2" if test_cookie.is_none() => test_cookie = Some(value),
            _ => return Err(ConfigError::MoogleMailKeyEnvParse { path: path.clone() }),
        }
    }
    let bot_cookie = bot_cookie.ok_or_else(|| ConfigError::MoogleMailKeyEnvMissing {
        path: path.clone(),
        key: "hv-ck-key1",
    })?;
    let test_cookie = test_cookie.ok_or_else(|| ConfigError::MoogleMailKeyEnvMissing {
        path: path.clone(),
        key: "hv-ck-key2",
    })?;
    Ok(MoogleMailKeyEnv {
        bot_cookie: SecretString::new("hv-ck-key1", bot_cookie)?,
        test_cookie: SecretString::new("hv-ck-key2", test_cookie)?,
    })
}

fn parse_mooglemail_keyenv_value(raw_value: &str) -> Option<String> {
    let value = if let Some(value) = raw_value.strip_prefix('"') {
        let value = value.strip_suffix('"')?;
        if value.contains(['"', '\\']) {
            return None;
        }
        value
    } else if let Some(value) = raw_value.strip_prefix('\'') {
        let value = value.strip_suffix('\'')?;
        if value.contains(['\'', '\\']) {
            return None;
        }
        value
    } else {
        if raw_value.contains(['"', '\'']) {
            return None;
        }
        raw_value
    };
    (!value.is_empty()
        && value.len() <= 4096
        && value == value.trim()
        && !value.chars().any(char::is_control))
    .then(|| value.to_owned())
}

fn load_admin_runtime_file(path: &Path) -> Result<AdminRuntimeDocument, ConfigError> {
    let contents = fs::read_to_string(path)
        .map_err(|source| ConfigError::RuntimeRead { path: path.to_owned(), source })?;
    let document: AdminRuntimeDocument = toml::from_str(&contents)
        .map_err(|_| ConfigError::RuntimeParse { path: path.to_owned() })?;
    document.validate().map_err(|error| ConfigError::InvalidRuntimeConfig {
        path: path.to_owned(),
        reason: error.to_string(),
    })?;
    Ok(document)
}

fn runtime_http_security(document: &AdminRuntimeDocument) -> HttpSecurityConfig {
    HttpSecurityConfig {
        allowed_origins: document.security.allowed_origins.clone(),
        allowed_origins_enabled: document.security.allowed_origins_enabled,
        login_rate_limit: document.security.login_rate_limit,
        mutation_rate_limit: document.security.mutation_rate_limit,
        rate_limit_window_seconds: document.security.rate_limit_window_seconds,
        cookie_secure: document.security.cookie_secure,
        request_timeout_seconds: document.security.request_timeout_seconds,
        trusted_proxy_cidrs: document.security.trusted_proxy_cidrs.clone(),
        trusted_unix_proxy: document.security.trusted_unix_proxy,
    }
}

fn runtime_session(document: &AdminRuntimeDocument) -> SessionConfig {
    SessionConfig {
        idle_ttl_seconds: document.session.idle_ttl_seconds,
        absolute_ttl_seconds: document.session.absolute_ttl_seconds,
        max_public_sessions: document.session.max_public_sessions,
    }
}
