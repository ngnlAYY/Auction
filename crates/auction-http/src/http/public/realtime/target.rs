//! Public 实时查询目标和旧版十进制参数解析。

use std::collections::BTreeMap;

use uuid::Uuid;

use auction_application::MAX_PUBLIC_EVENT_PAGE_SIZE;

pub(super) const DEFAULT_PUBLIC_EVENT_PAGE_SIZE: i64 = 100;

/// v1 实时边界的明确目标。单拍卖合同沿用旧版，发现流只能通过显式 scope 扩展进入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RealtimeTarget {
    Auction(Uuid),
    Discovery,
}

impl RealtimeTarget {
    pub(super) const fn auction_id(self) -> Option<Uuid> {
        match self {
            Self::Auction(auction_id) => Some(auction_id),
            Self::Discovery => None,
        }
    }

    pub(super) fn stream_key(self) -> String {
        match self {
            Self::Auction(auction_id) => format!("auction:{auction_id}"),
            Self::Discovery => "auction:discovery".to_owned(),
        }
    }
}

pub(super) fn parse_http_target(query: &BTreeMap<&str, String>) -> Result<RealtimeTarget, ()> {
    match (query.get("auction_id"), query.get("scope")) {
        (Some(auction_id), None) => parse_auction_id(auction_id).map(RealtimeTarget::Auction),
        (None, Some(scope)) if scope == "auctions" => Ok(RealtimeTarget::Discovery),
        _ => Err(()),
    }
}

pub(super) fn parse_websocket_target(query: &BTreeMap<&str, String>) -> Result<RealtimeTarget, ()> {
    if query.len() != 1 {
        return Err(());
    }
    parse_http_target(query)
}

/// 旧版只接受无符号规范十进制：`+1`、`01`、空串和超出 i64 的值都必须失败关闭。
pub(super) fn parse_nonnegative(value: Option<&String>, fallback: i64) -> Result<i64, ()> {
    let Some(value) = value else {
        return Ok(fallback);
    };
    let parsed = value.parse::<u64>().map_err(|_| ())?;
    if parsed > i64::MAX as u64 || parsed.to_string() != *value {
        return Err(());
    }
    Ok(parsed as i64)
}

/// 解析旧版规范正十进制页大小，并保留应用端口的相同上限。
pub(super) fn parse_page_size(value: Option<&String>) -> Result<i64, ()> {
    let Some(value) = value else {
        return Ok(DEFAULT_PUBLIC_EVENT_PAGE_SIZE);
    };
    let parsed = value.parse::<u64>().map_err(|_| ())?;
    if parsed == 0 || parsed > MAX_PUBLIC_EVENT_PAGE_SIZE as u64 || parsed.to_string() != *value {
        return Err(());
    }
    Ok(parsed as i64)
}

fn parse_auction_id(value: &str) -> Result<Uuid, ()> {
    super::super::parse_public_uuid(value)
}
