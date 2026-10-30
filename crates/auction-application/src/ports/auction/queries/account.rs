//! Public 当前账户的竞价、代理授权和历史投影端口。

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::ApplicationFuture;

/// 账户竞价读取允许的最大页大小。
pub const MAX_PUBLIC_ACCOUNT_PAGE_SIZE: i64 = 200;

/// 账户竞价兼容页允许的最大 offset。
pub const MAX_PUBLIC_ACCOUNT_OFFSET: i64 = 10_000;

/// 旧账户接口的默认页大小。
pub const DEFAULT_PUBLIC_ACCOUNT_PAGE_SIZE: i64 = 20;

/// 账户竞价与代理历史的一张数字页。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicAccountPageRequest {
    pub limit: i64,
    pub offset: i64,
}

impl PublicAccountPageRequest {
    /// 拒绝无界页大小、负 offset 和过深的兼容分页。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.limit < 1 || self.limit > MAX_PUBLIC_ACCOUNT_PAGE_SIZE || self.offset < 0 {
            return Err("公开账户分页参数无效");
        }
        if self.offset > MAX_PUBLIC_ACCOUNT_OFFSET {
            return Err("公开账户分页 offset 超出上限");
        }
        Ok(())
    }
}

/// 当前用户竞价和代理历史的独立分页请求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicMyBidsRequest {
    pub bids: PublicAccountPageRequest,
    pub proxies: PublicAccountPageRequest,
}

impl PublicMyBidsRequest {
    /// 在应用端口边界同时校验两张账户表的分页。
    pub fn validate(&self) -> Result<(), &'static str> {
        self.bids.validate()?;
        self.proxies.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicMyBid {
    pub id: Uuid,
    pub auction_id: Uuid,
    pub lot_id: Uuid,
    pub lot_number: i64,
    pub amount: i64,
    pub accepted_state_version: i64,
    pub fact_index: i32,
    pub mechanism: String,
    pub created_at: DateTime<Utc>,
    pub equipment_name: String,
    pub equipment_category: String,
    pub leading: bool,
}

/// 代理产生的最近一次公开竞价，不携带代理上限等 owner-only 字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicProxyBid {
    pub id: Uuid,
    pub auction_id: Uuid,
    pub lot_id: Uuid,
    pub lot_number: i64,
    pub amount: i64,
    pub accepted_state_version: i64,
    pub fact_index: i32,
    pub mechanism: String,
    pub created_at: DateTime<Utc>,
    pub leading: bool,
}

/// 代理授权历史中的一条 append-only 转换事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicProxyTransition {
    pub transition_number: i64,
    pub kind: String,
    pub old_maximum_amount: Option<i64>,
    pub new_maximum_amount: i64,
    pub old_mode: Option<String>,
    pub new_mode: String,
    pub old_status: Option<String>,
    pub new_status: String,
    pub reason: Option<String>,
    pub resulting_state_version: i64,
    pub created_at: DateTime<Utc>,
}

/// owner-only 代理授权的派生操作能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicProxyCapabilities {
    pub can_lower: bool,
    pub waiting: bool,
    pub at_maximum: bool,
}

/// 当前用户在指定公开 lot 上拥有的代理授权投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicOwnedProxy {
    pub authorization_id: Uuid,
    pub auction_id: Uuid,
    pub lot_id: Uuid,
    pub user_id: Uuid,
    pub lot_number: i64,
    pub authorization_sequence: i64,
    pub usage_number: i64,
    pub quota_used: i64,
    pub quota_limit: i64,
    pub mode: String,
    pub maximum_amount: i64,
    pub status: String,
    pub terminal_reason: Option<String>,
    pub state_version: i64,
    pub transitions: Vec<PublicProxyTransition>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub equipment_name: String,
    pub equipment_category: String,
    pub bid: Option<PublicProxyBid>,
    pub invalid_reason: Option<String>,
    pub capabilities: PublicProxyCapabilities,
}

/// 当前用户竞价和代理历史的结果页。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicMyBidsPage {
    pub bids: Vec<PublicMyBid>,
    pub proxies: Vec<PublicOwnedProxy>,
    pub bid_total: i64,
    pub proxy_total: i64,
}

pub trait PublicAccountReader: Send + Sync {
    fn list_public_user_bids(
        &self,
        user_id: Uuid,
        request: PublicMyBidsRequest,
    ) -> ApplicationFuture<'_, PublicMyBidsPage>;

    fn public_user_proxy(
        &self,
        lot_id: Uuid,
        user_id: Uuid,
    ) -> ApplicationFuture<'_, Option<PublicOwnedProxy>>;
}
