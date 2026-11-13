//! IP 网段（CIDR）的解析、规范文本与归属判断。

use std::{
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    str::FromStr,
};

use thiserror::Error;

/// 一个 IPv4 或 IPv6 网段。解析时清除主机位，因此 `Display` 总是规范文本；
/// 调用方用“显示结果与输入逐字相等”判定输入是否规范。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpCidr {
    network: IpAddr,
    prefix_length: u8,
}

/// CIDR 文本无法解析；不回显输入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("CIDR is malformed")]
pub struct IpCidrParseError;

impl IpCidr {
    /// 前缀长度；0 表示该地址族的全部地址。
    pub const fn prefix_length(&self) -> u8 {
        self.prefix_length
    }

    /// 地址是否属于该网段；地址族不同一律不属于，IPv4 映射地址不做隐式换算。
    pub const fn contains(&self, address: IpAddr) -> bool {
        match (self.network, address) {
            (IpAddr::V4(network), IpAddr::V4(address)) => {
                address.to_bits() & ipv4_mask(self.prefix_length) == network.to_bits()
            }
            (IpAddr::V6(network), IpAddr::V6(address)) => {
                address.to_bits() & ipv6_mask(self.prefix_length) == network.to_bits()
            }
            _ => false,
        }
    }
}

impl FromStr for IpCidr {
    type Err = IpCidrParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (address, prefix) = value.split_once('/').ok_or(IpCidrParseError)?;
        // `u8::from_str` 接受前导 `+`；前缀只允许十进制数字。
        if prefix.is_empty() || !prefix.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(IpCidrParseError);
        }
        let Ok(prefix_length) = prefix.parse::<u8>() else {
            return Err(IpCidrParseError);
        };
        let network = match address.parse::<IpAddr>() {
            Ok(IpAddr::V4(address)) if u32::from(prefix_length) <= Ipv4Addr::BITS => {
                IpAddr::V4(Ipv4Addr::from_bits(address.to_bits() & ipv4_mask(prefix_length)))
            }
            Ok(IpAddr::V6(address)) if u32::from(prefix_length) <= Ipv6Addr::BITS => {
                IpAddr::V6(Ipv6Addr::from_bits(address.to_bits() & ipv6_mask(prefix_length)))
            }
            // 地址无法解析或前缀超出该地址族的位数。
            _ => return Err(IpCidrParseError),
        };
        Ok(Self { network, prefix_length })
    }
}

impl fmt::Display for IpCidr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.network, self.prefix_length)
    }
}

const fn ipv4_mask(prefix_length: u8) -> u32 {
    match u32::MAX.checked_shl(Ipv4Addr::BITS - prefix_length as u32) {
        Some(mask) => mask,
        None => 0,
    }
}

const fn ipv6_mask(prefix_length: u8) -> u128 {
    match u128::MAX.checked_shl(Ipv6Addr::BITS - prefix_length as u32) {
        Some(mask) => mask,
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::{IpCidr, IpCidrParseError};

    fn cidr(value: &str) -> IpCidr {
        value.parse().unwrap()
    }

    fn address(value: &str) -> IpAddr {
        value.parse().unwrap()
    }

    #[test]
    fn canonical_networks_round_trip_and_host_bits_are_cleared() {
        for value in ["10.0.0.0/8", "192.0.2.128/25", "203.0.113.7/32", "2001:db8::/32", "::1/128"]
        {
            assert_eq!(cidr(value).to_string(), value);
        }
        assert_eq!(cidr("10.1.2.3/8").to_string(), "10.0.0.0/8");
        assert_eq!(cidr("192.0.2.255/31").to_string(), "192.0.2.254/31");
        assert_eq!(cidr("2001:db8:ffff::1/33").to_string(), "2001:db8:8000::/33");
        // 非规范写法可以解析，但显示结果与输入不同，调用方据此拒绝。
        assert_eq!(cidr("10.0.0.0/08").to_string(), "10.0.0.0/8");
        assert_eq!(cidr("2001:0db8::/32").to_string(), "2001:db8::/32");
        assert_eq!(cidr("198.51.100.1/0").prefix_length(), 0);
    }

    #[test]
    fn malformed_text_fails_closed() {
        for value in [
            "",
            "/",
            "10.0.0.0",
            "10.0.0.0/",
            "10.0.0.0/33",
            "::/129",
            "10.0.0.0/+8",
            "10.0.0.0/-1",
            "10.0.0.0/ 8",
            "10.0.0.0/8 ",
            " 10.0.0.0/8",
            "10.0.0.0/8/8",
            "10.0.0/8",
            "010.0.0.0/8",
            "fe80::1%eth0/64",
            "example.com/8",
            "10.0.0.0/256",
        ] {
            assert_eq!(value.parse::<IpCidr>(), Err(IpCidrParseError), "{value:?}");
        }
    }

    #[test]
    fn membership_follows_prefix_boundaries_within_one_address_family() {
        let private = cidr("10.0.0.0/8");
        assert!(private.contains(address("10.0.0.0")));
        assert!(private.contains(address("10.255.255.255")));
        assert!(!private.contains(address("9.255.255.255")));
        assert!(!private.contains(address("11.0.0.0")));

        let half = cidr("192.0.2.128/25");
        assert!(half.contains(address("192.0.2.128")));
        assert!(half.contains(address("192.0.2.255")));
        assert!(!half.contains(address("192.0.2.127")));

        let host = cidr("203.0.113.7/32");
        assert!(host.contains(address("203.0.113.7")));
        assert!(!host.contains(address("203.0.113.6")));

        let documentation = cidr("2001:db8::/32");
        assert!(documentation.contains(address("2001:db8:ffff:ffff:ffff:ffff:ffff:ffff")));
        assert!(!documentation.contains(address("2001:db9::")));
        assert!(cidr("::1/128").contains(address("::1")));

        assert!(cidr("0.0.0.0/0").contains(address("198.51.100.1")));
        assert!(cidr("::/0").contains(address("2001:db8::1")));
        // 地址族不同不匹配，IPv4 映射网段也不隐式覆盖 IPv4 地址。
        assert!(!cidr("0.0.0.0/0").contains(address("::1")));
        assert!(!cidr("::/0").contains(address("10.0.0.1")));
        assert!(!cidr("::ffff:10.0.0.0/104").contains(address("10.0.0.1")));
        assert!(cidr("::ffff:10.0.0.0/104").contains(address("::ffff:10.0.0.1")));
    }
}
