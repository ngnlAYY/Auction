//! 校验直接交给 HTTP 连接器的 DNS 地址集合，拒绝非公网目标。

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use super::SOURCE_HOST;

pub(super) struct EquipmentDns;

impl Resolve for EquipmentDns {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            if name.as_str() != SOURCE_HOST {
                return Err(resolution_rejected().into());
            }
            let addresses = tokio::net::lookup_host((name.as_str(), 0)).await?;
            // 这些是连接器实际收到的 SocketAddr。HTTPS URI 仍持有固定主机，用于 Host 和 TLS 验证。
            checked_addresses(addresses, public_address).map_err(Into::into)
        })
    }
}

pub(super) fn checked_addresses(
    source: impl IntoIterator<Item = SocketAddr>,
    allowed: impl Fn(IpAddr) -> bool,
) -> io::Result<Addrs> {
    let addresses: Vec<_> = source.into_iter().collect();
    // 混合应答也失败关闭，避免把部分异常解析当成已验证的来源。
    if addresses.is_empty() || addresses.iter().any(|address| !allowed(address.ip())) {
        return Err(resolution_rejected());
    }
    Ok(Box::new(addresses.into_iter()))
}

fn resolution_rejected() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "装备来源地址校验未通过")
}

pub(super) fn public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => public_ipv4(ip),
        IpAddr::V6(ip) => public_ipv6(ip),
    }
}

fn public_ipv4(address: Ipv4Addr) -> bool {
    !matches!(
        address.octets(),
        [0 | 10 | 127 | 224..=255, ..]
            | [100, 64..=127, ..]
            | [169, 254, ..]
            | [172, 16..=31, ..]
            | [192, 0, 0 | 2, _]
            | [192, 88, 99, _]
            | [192, 168, ..]
            | [198, 18 | 19, ..]
            | [198, 51, 100, _]
            | [203, 0, 113, _]
    )
}

fn public_ipv6(address: Ipv6Addr) -> bool {
    // 仅接受当前全球单播分配空间，排除其中的特殊协议、过渡隧道与文档前缀。
    matches!(address.segments(), [0x2000..=0x3ffe, ..] | [0x3fff, 0x1000..=0xffff, ..])
        && !matches!(address.segments(), [0x2001, 0..=0x01ff | 0x0db8, ..] | [0x2002, ..])
}
