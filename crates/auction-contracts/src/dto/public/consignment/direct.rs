//! 免登录非寄售登记的输入、回执查询结果与挂单进度。

use std::fmt;

use auction_time::Timestamp;
use uuid::Uuid;

use crate::deserialize_required_option;
use crate::dto::public::consignment::commands::ListingDisposition;
use crate::dto::public::consignment::commands::StoredCommandResult;
use crate::dto::public::consignment::scalar::ListingPrice;
use crate::dto::public::consignment::scalar::ListingVersion;
use crate::dto::public::consignment::states::ListingMode;
use crate::dto::public::consignment::states::ListingReason;
use crate::dto::public::consignment::states::ListingState;
use crate::dto::public::consignment::wire::json_object;
use crate::dto::public::consignment::wire::timestamp;
use crate::dto::public::consignment::wire::uuid;
use crate::dto::public::consignment::write::write_object;

write_object! {
    /// 免登录非寄售登记请求。不变量 1、6、7：稳定命令 ID、严格价格；模式固定为非寄售，
    /// 自填游戏角色名由领域校验，装备链接只在输入中传递。
    #[derive(Clone, PartialEq, Eq)]
    pub struct RegisterDirectListing {
        pub command_id: Uuid => ("uuid", "必须为规范小写的非零 UUID"),
        pub seller_game_name: String => ("serde::Deserialize::deserialize", "必须为字符串"),
        pub equipment_url: String => ("serde::Deserialize::deserialize", "必须为字符串"),
        pub price: ListingPrice => ("serde::Deserialize::deserialize", "必须为范围内的 JSON 整数（1–1000000000）"),
    }
}

impl fmt::Debug for RegisterDirectListing {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RegisterDirectListing")
            .field("command_id", &self.command_id)
            .field("seller_game_name", &self.seller_game_name)
            .field("equipment_url", &"[REDACTED]")
            .field("price", &self.price)
            .finish()
    }
}

json_object! {
    ()
    /// 免登录登记受理后挂单的当前进度；不含卖家名、费率与可执行动作。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct DirectListingProgress {
        #[serde(deserialize_with = "uuid")]
        pub listing_id: Uuid,
        pub version: ListingVersion,
        pub effective_state: ListingState,
        #[serde(deserialize_with = "deserialize_required_option")]
        pub effective_reason: Option<ListingReason>,
        #[serde(deserialize_with = "timestamp")]
        pub expires_at: Timestamp,
    } => validate
}

impl DirectListingProgress {
    fn validate(&self) -> Result<(), &'static str> {
        // 不变量 6：免登录挂单只有非寄售模式，寄售专属状态不可能出现。
        if !self.effective_state.matches_mode(ListingMode::Direct) {
            return Err("免登录挂单的状态与非寄售模式不一致");
        }
        Ok(())
    }
}

json_object! {
    ()
    /// 凭原命令 ID 查询免登录登记：首次决定原样保留，受理时附挂单当前进度。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct DirectCommandResult {
        pub result: StoredCommandResult,
        #[serde(deserialize_with = "deserialize_required_option")]
        pub listing: Option<DirectListingProgress>,
    } => validate
}

impl DirectCommandResult {
    fn validate(&self) -> Result<(), &'static str> {
        // 不变量 1、6：只有已受理的登记有挂单，且挂单必须是回执定位的那一个。
        let receipt = &self.result.receipt;
        let consistent = match (&self.listing, receipt.disposition) {
            (Some(listing), ListingDisposition::Registered) => {
                receipt.listing_id == Some(listing.listing_id)
            }
            (None, ListingDisposition::Rejected) => true,
            _ => false,
        };
        if !consistent {
            return Err("免登录登记回执与挂单进度不一致");
        }
        Ok(())
    }
}
