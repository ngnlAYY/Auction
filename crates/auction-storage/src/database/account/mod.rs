//! Public 账户竞价与代理历史的只读适配器；projection 只在同一快照内拼装当前用户的公开拍卖/lot。
//! 代理 capability 按权威 lot 状态派生，HTTP 层不接触数据库行或内部代理状态机。

use auction_application::{
    ApplicationError, ApplicationFuture, PublicAccountReader, PublicMyBid, PublicMyBidsPage,
    PublicMyBidsRequest, PublicOwnedProxy, PublicProxyBid, PublicProxyCapabilities,
    PublicProxyTransition,
};
use auction_domain::{AuctionRules, BidAmount};
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use super::DatabaseStore;

impl PublicAccountReader for DatabaseStore {
    fn list_public_user_bids(
        &self,
        user_id: Uuid,
        request: PublicMyBidsRequest,
    ) -> ApplicationFuture<'_, PublicMyBidsPage> {
        Box::pin(async move {
            validate_account_request(user_id, request)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => list_postgres(store, user_id, request).await,
            }
        })
    }

    fn public_user_proxy(
        &self,
        lot_id: Uuid,
        user_id: Uuid,
    ) -> ApplicationFuture<'_, Option<PublicOwnedProxy>> {
        Box::pin(async move {
            if lot_id.is_nil() || user_id.is_nil() {
                return Err(storage_error("公开账户代理查询标识无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => proxy_postgres(store, lot_id, user_id).await,
            }
        })
    }
}

#[derive(Debug)]
struct ProxyBase {
    authorization_id: Uuid,
    auction_id: Uuid,
    lot_id: Uuid,
    user_id: Uuid,
    lot_number: i64,
    authorization_sequence: i64,
    usage_number: i64,
    quota_used: i64,
    quota_limit: i64,
    policy_bid_increment_ppm: i64,
    policy_minimum_increment: i64,
    mode: String,
    maximum_amount: i64,
    status: String,
    terminal_reason: Option<String>,
    state_version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    equipment_name: String,
    equipment_category: String,
    start_price: i64,
    current_price: Option<i64>,
    material: bool,
    leader_user_id: Option<Uuid>,
    slow_authorization_id: Option<Uuid>,
}

#[cfg(feature = "postgres")]
use super::PostgresStore;

mod mapping;
#[cfg(feature = "postgres")]
mod postgres;

use mapping::*;
#[cfg(feature = "postgres")]
use postgres::{list_postgres, proxy_postgres};
