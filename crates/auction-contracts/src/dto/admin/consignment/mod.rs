//! Admin 寄售查询、碰撞记录与下架请求。

mod listing_v2;
mod loss;
mod order_v2;
mod quarantine;

pub use listing_v2::*;
pub use loss::*;
pub use order_v2::*;
pub use quarantine::*;

use std::fmt;

use auction_time::Timestamp;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serializer;
use serde::de::Error;
use uuid::Uuid;

use crate::ListingVersion;
use crate::OwnedListing;
use crate::dto::public::consignment::wire::checked_text;
use crate::dto::public::consignment::wire::json_object;
use crate::dto::public::consignment::wire::timestamp;
use crate::dto::public::consignment::wire::uuid;
use crate::dto::public::consignment::write::redact_error;
use crate::dto::public::consignment::write::write_object;

const MAX_DELIST_REASON_BYTES: usize = 256;
const MAX_AUCTION_CONFLICTS: usize = 50;

json_object! {
    ()
    /// Admin 列表记录。卖家内部标识和下架说明仅给运营会话。
    /// 私有说明沿用下架请求的凭据筛查，调试输出统一隐去说明。
    #[derive(Clone, PartialEq)]
    pub struct AdminListing {
        pub listing: OwnedListing,
        #[serde(deserialize_with = "uuid")]
        pub seller_user_id: Uuid,
        pub seller_name: String,
        #[serde(deserialize_with = "optional_delist_reason")]
        pub admin_delist_reason: Option<String>,
        pub can_delist: bool,
    } => validate
}

impl AdminListing {
    fn validate(&self) -> Result<(), &'static str> {
        if !self.listing.allowed_actions.is_empty() {
            return Err("Admin 挂单投影不得携带卖家操作");
        }
        if self.can_delist && !self.listing.effective_state.is_active() {
            return Err("已终结的挂单不得提供 Admin 下架操作");
        }
        if let Some(reason) = &self.admin_delist_reason {
            validate_delist_reason(reason)?;
        }
        Ok(())
    }
}

impl fmt::Debug for AdminListing {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminListing")
            .field("listing", &self.listing)
            .field("seller_user_id", &self.seller_user_id)
            .field("seller_name", &self.seller_name)
            .field("admin_delist_reason", &self.admin_delist_reason.as_ref().map(|_| "[REDACTED]"))
            .field("can_delist", &self.can_delist)
            .finish()
    }
}

json_object! {
    ()
    /// Admin 详情及至多 50 条最近碰撞。
    /// 服务端保证 `detected_at DESC, lot_id DESC`，解码器不重排也不校验顺序。
    #[derive(Debug, Clone, PartialEq)]
    pub struct AdminListingDetail {
        pub record: AdminListing,
        pub conflicts: Vec<ListingAuctionConflict>,
        pub conflicts_truncated: bool,
    } => validate
}

impl AdminListingDetail {
    fn validate(&self) -> Result<(), &'static str> {
        if self.conflicts.len() > MAX_AUCTION_CONFLICTS {
            return Err("寄售详情的碰撞记录超出上限");
        }
        if self.conflicts_truncated && self.conflicts.len() != MAX_AUCTION_CONFLICTS {
            return Err("标记截断时必须返回达到上限的碰撞记录");
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

write_object! {
    /// 下架请求。不变量 1、6、7：携带稳定 ID、预期版本及私有说明。
    /// 说明只进 Admin 私有视图。与应用层一致，在全文按 ASCII 大小写无关匹配
    /// `/equip/`、`/equip?`、`key=`，并拒绝独立的 10 位十六进制片段和
    /// 恰为 43 个 ASCII 字母、数字、连字符或下划线的片段。
    #[derive(Clone, PartialEq, Eq)]
    pub struct AdminDelistListing {
        pub command_id: Uuid => ("uuid", "必须为规范小写的非零 UUID"),
        pub expected_version: ListingVersion => ("serde::Deserialize::deserialize", "必须为正的安全 JSON 整数"),
        #[serde(serialize_with = "serialize_delist_reason")]
        pub reason: String => ("delist_reason", "必须为不含装备链接或已知凭据形态的规范单行文本（1–256 字节）"),
    }
}

fn delist_reason<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let reason = String::deserialize(deserializer)?;
    validate_delist_reason(&reason).map_err(D::Error::custom)?;
    Ok(reason)
}

fn optional_delist_reason<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    // 保留显式 null，字段类型错误同样不能回显输入。
    Option::<String>::deserialize(deserializer)
        .map_err(|error| redact_error(error, "admin_delist_reason 字段必须为字符串或 null"))
}

fn serialize_delist_reason<S: Serializer>(reason: &str, serializer: S) -> Result<S::Ok, S::Error> {
    validate_delist_reason(reason).map_err(serde::ser::Error::custom)?;
    serializer.serialize_str(reason)
}

fn validate_delist_reason(reason: &str) -> Result<(), &'static str> {
    checked_text::<MAX_DELIST_REASON_BYTES>(reason)?;
    // checked_text 已拒绝控制字符，其中包括 U+0085。
    if reason.contains(['\u{2028}', '\u{2029}']) {
        return Err("运营下架说明不得包含行分隔符");
    }
    if contains_equipment_link(reason)
        || reason.split(|ch: char| !ch.is_ascii_alphanumeric()).any(is_equipment_key)
        || reason
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_')))
            .any(|token| token.len() == 43)
    {
        return Err("运营下架说明不得包含装备链接或已知凭据形态");
    }
    Ok(())
}

fn is_equipment_key(value: &str) -> bool {
    value.len() == 10 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn contains_equipment_link(reason: &str) -> bool {
    [b"/equip/".as_slice(), b"/equip?".as_slice(), b"key=".as_slice()].into_iter().any(|fragment| {
        reason.as_bytes().windows(fragment.len()).any(|part| part.eq_ignore_ascii_case(fragment))
    })
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
