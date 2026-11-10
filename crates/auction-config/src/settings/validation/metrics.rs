//! 指标监听校验；关闭开关也不能留下非法或冲突的地址。

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use super::super::{ConfigError, MetricsConfig, Role};

pub(crate) fn validate_metrics_listener(
    role: Role,
    metrics: MetricsConfig,
    uses_unix_listener: bool,
    listen_addr: Option<SocketAddr>,
) -> Result<(), ConfigError> {
    let addresses = [
        metrics.public_loopback_listen_address,
        metrics.admin_loopback_listen_address,
        metrics.worker_loopback_listen_address,
    ];
    for (index, address) in addresses.iter().enumerate() {
        let Some(address) = address else {
            continue;
        };
        if address.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) || address.port() == 0 {
            return Err(ConfigError::InvalidMetricsConfig("附加指标地址须为 127.0.0.1 与非零端口"));
        }
        if addresses[..index].contains(&Some(*address)) {
            return Err(ConfigError::InvalidMetricsConfig("三角色附加指标地址不能重复"));
        }
        // 共享 common 中任一出口都不能占用本角色的业务端口。
        if let Some(business) = listen_addr
            && address.port() == business.port()
            && (business.ip().to_canonical() == address.ip()
                || business.ip().to_canonical().is_unspecified())
        {
            return Err(ConfigError::InvalidMetricsConfig("附加指标与业务 TCP 监听冲突"));
        }
    }
    if !metrics.enabled || role == Role::Worker || uses_unix_listener {
        return Ok(());
    }
    if listen_addr.is_some_and(|address| address.ip().is_loopback()) {
        return Ok(());
    }
    Err(ConfigError::InvalidMetricsConfig(
        "Public/Admin 启用 metrics 时必须使用 loopback TCP 或 Unix listener",
    ))
}
