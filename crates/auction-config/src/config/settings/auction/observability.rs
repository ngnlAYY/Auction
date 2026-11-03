//! Metrics、tracing 和 realtime 配置。

use std::net::SocketAddr;

use serde::Deserialize;

use super::Role;

/// 进程本地 Prometheus 指标导出策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MetricsConfig {
    /// 是否注册并暴露固定 `/metrics` scrape endpoint；省略时关闭。
    pub enabled: bool,
    /// 业务指标路由与三角色附加出口的转发拒绝策略，默认开启。
    /// 仅由部署配置持有，Admin runtime 只能覆盖 enabled，不得覆盖该安全策略。
    pub reject_forwarded_requests: bool,
    /// Public 附加指标地址；省略即关闭，仅接受 127.0.0.1 与非零端口。
    pub public_loopback_listen_address: Option<SocketAddr>,
    /// Admin 附加指标地址；不受 Admin runtime 覆盖。
    pub admin_loopback_listen_address: Option<SocketAddr>,
    /// Worker 附加指标地址；独立于 control socket，省略即关闭。
    pub worker_loopback_listen_address: Option<SocketAddr>,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            reject_forwarded_requests: true,
            public_loopback_listen_address: None,
            admin_loopback_listen_address: None,
            worker_loopback_listen_address: None,
        }
    }
}

impl MetricsConfig {
    /// 只在有效指标开关开启时选择本角色的附加出口。
    pub fn loopback_listen_address(self, role: Role) -> Option<SocketAddr> {
        if !self.enabled {
            return None;
        }
        match role {
            Role::Public => self.public_loopback_listen_address,
            Role::Admin => self.admin_loopback_listen_address,
            Role::Worker => self.worker_loopback_listen_address,
        }
    }
}

/// Public/Worker 进程的 OTLP/HTTP tracing 非敏感运行参数。
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
