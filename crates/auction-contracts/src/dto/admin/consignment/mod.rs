//! Admin 寄售查询、碰撞记录与下架请求。

use std::fmt;

use auction_time::Timestamp;
use serde::de::Error;
use uuid::Uuid;

use crate::OwnedListing;
use crate::dto::public::consignment::wire::SAFE_INTEGER_MAX;
use crate::dto::public::consignment::wire::integer;
use crate::dto::public::consignment::wire::json_object;
use crate::dto::public::consignment::wire::optional_text;
use crate::dto::public::consignment::wire::text;
use crate::dto::public::consignment::wire::timestamp;
use crate::dto::public::consignment::wire::uuid;

const MAX_DELIST_REASON_BYTES: usize = 256;
const MAX_AUCTION_CONFLICTS: usize = 50;

json_object! {
    ()
    /// Admin 列表记录。卖家内部标识和下架说明仅给运营会话。
    #[derive(Debug, Clone, PartialEq)]
    pub struct AdminListing {
        pub listing: OwnedListing,
        #[serde(deserialize_with = "uuid")]
        pub seller_user_id: Uuid,
        pub seller_name: String,
        #[serde(deserialize_with = "optional_text::<MAX_DELIST_REASON_BYTES, _>")]
        pub admin_delist_reason: Option<String>,
        pub can_delist: bool,
    } => validate
}

impl AdminListing {
    fn validate<E: Error>(&self) -> Result<(), E> {
        if !self.listing.allowed_actions.is_empty() {
            return Err(E::custom("Admin 挂单投影不得携带卖家操作"));
        }
        Ok(())
    }
}

json_object! {
    ()
    /// Admin 详情及最近的实际拍卖碰撞。
    #[derive(Debug, Clone, PartialEq)]
    pub struct AdminListingDetail {
        pub record: AdminListing,
        pub conflicts: Vec<ListingAuctionConflict>,
        pub conflicts_truncated: bool,
    } => validate
}

impl AdminListingDetail {
    fn validate<E: Error>(&self) -> Result<(), E> {
        if self.conflicts.len() > MAX_AUCTION_CONFLICTS {
            return Err(E::custom("寄售详情的碰撞记录超出上限"));
        }
        Ok(())
    }
}

json_object! {
    ()
    /// 与该挂单碰撞的一条已提交拍卖事实。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ListingAuctionConflict {
        #[serde(deserialize_with = "uuid")]
        pub lot_id: Uuid,
        #[serde(deserialize_with = "timestamp")]
        pub detected_at: Timestamp,
    }
}

json_object! {
    ()
    /// 下架请求。不变量 1、6、7：携带稳定 ID、预期版本及私有说明。
    /// 合同没有凭据文本的闭集，写入口须校验说明不含装备链接或凭据后才可持久化。
    #[derive(Clone, PartialEq, Eq)]
    pub struct AdminDelistListing {
        #[serde(deserialize_with = "uuid")]
        pub command_id: Uuid,
        #[serde(deserialize_with = "integer::<1, SAFE_INTEGER_MAX, _>")]
        pub expected_version: i64,
        #[serde(deserialize_with = "text::<MAX_DELIST_REASON_BYTES, _>")]
        pub reason: String,
    }
}

impl fmt::Debug for AdminDelistListing {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminDelistListing")
            .field("command_id", &self.command_id)
            .field("expected_version", &self.expected_version)
            .field("reason", &"[REDACTED]")
            .finish()
    }
}
