//! 固定第三方装备来源的装配与查询端口。

mod dns;
mod http;
mod json;
mod snapshot;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{
    ListingEquipmentLookup, ListingEquipmentLookupError, ListingLookupSnapshot,
};
use auction_application::core::TransactionFuture;
use auction_domain::EquipmentKey;
use auction_domain::consignment::ListingEquipmentIdentity;
use auction_time::Timestamp;
use reqwest::{Client, ClientBuilder};
use thiserror::Error;
use tokio::time::{Instant, sleep_until};
use tracing::instrument::WithSubscriber;
use url::Url;

use self::dns::EquipmentDns;
use crate::outbound::restricted_client_builder;

const SOURCE_HOST: &str = "hvdata.gisadan.dev";
const SOURCE_ENDPOINT: &str = "https://hvdata.gisadan.dev/equip";
const TOTAL_HTTP_LIMIT: Duration = Duration::from_secs(5);
const MAXIMUM_CONFIGURED_BYTES: usize = 262_144;

/// 固定来源装配失败，只暴露安全分类。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum HvDataConfigurationError {
    /// 解压后字节上限越过配置范围。
    #[error("寄售装备来源响应上限无效")]
    ResponseLimit,
    /// 固定 HTTPS 客户端不可用。
    #[error("无法初始化寄售装备来源 HTTPS 客户端")]
    Client,
}

/// 按固定的 `hvdata-equip-v1` 协议读取寄售装备。
pub struct HvDataEquipmentReader {
    client: Client,
    endpoint: Url,
    maximum_response_bytes: usize,
}

impl HvDataEquipmentReader {
    /// 由 Worker 传入已校验的解压后字节上限；请求地址和传输策略在此固定。
    pub fn new(maximum_response_bytes: usize) -> Result<Self, HvDataConfigurationError> {
        if !(4096..=MAXIMUM_CONFIGURED_BYTES).contains(&maximum_response_bytes) {
            return Err(HvDataConfigurationError::ResponseLimit);
        }
        let Ok(endpoint) = Url::parse(SOURCE_ENDPOINT) else {
            return Err(HvDataConfigurationError::Client);
        };
        let Ok(client) = equipment_client_builder().dns_resolver(Arc::new(EquipmentDns)).build()
        else {
            return Err(HvDataConfigurationError::Client);
        };
        Ok(Self { client, endpoint, maximum_response_bytes })
    }

    async fn lookup_until(
        &self,
        identity: &ListingEquipmentIdentity,
        key: &EquipmentKey,
        deadline: Timestamp,
    ) -> Result<ListingLookupSnapshot, ListingEquipmentLookupError> {
        let started = Instant::now();
        let Ok(remaining) = deadline.signed_duration_since(Timestamp::now()).to_std() else {
            return Err(ListingEquipmentLookupError::SourceTimeout);
        };
        let end = started + remaining.min(TOTAL_HTTP_LIMIT);
        if Instant::now() >= end {
            return Err(ListingEquipmentLookupError::SourceTimeout);
        }
        let result = tokio::select! {
            biased;
            () = sleep_until(end) => return Err(ListingEquipmentLookupError::SourceTimeout),
            result = self.exchange(identity, key) => result,
        };
        // 超时器无法抢占同步 JSON 解码，返回前再次以同一单调时钟检查截止。
        if Instant::now() >= end {
            return Err(ListingEquipmentLookupError::SourceTimeout);
        }
        result
    }
}

fn equipment_client_builder() -> ClientBuilder {
    // HTTP/1 限制压缩头部和连接内部诊断涉及的传输面；禁用连接字节日志和空闲复用。
    restricted_client_builder()
        .http1_only()
        .connection_verbose(false)
        .referer(false)
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .pool_max_idle_per_host(0)
}

impl ListingEquipmentLookup for HvDataEquipmentReader {
    fn lookup<'a>(
        &'a self,
        identity: &'a ListingEquipmentIdentity,
        key: &'a EquipmentKey,
        deadline: Timestamp,
    ) -> TransactionFuture<'a, ListingLookupSnapshot, ListingEquipmentLookupError> {
        // 不变量 7：隔离传输 future 的诊断，结果只交回安全分类。
        Box::pin(
            self.lookup_until(identity, key, deadline).with_subscriber(tracing::Dispatch::none()),
        )
    }
}
