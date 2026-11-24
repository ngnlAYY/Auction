//! 本人寄件指引、预约签发请求与 201 回执。

use std::fmt;

use auction_time::Timestamp;
use uuid::Uuid;

use crate::dto::public::consignment::listing_v2::ListingStateV2;
use crate::dto::public::consignment::receipt::ConsignmentDispositionV2;
use crate::dto::public::consignment::receipt::ConsignmentOperationV2;
use crate::dto::public::consignment::scalar::ListingVersion;
use crate::dto::public::consignment::states::ListingWorld;
use crate::dto::public::consignment::wire::json_object;
use crate::dto::public::consignment::wire::text;
use crate::dto::public::consignment::wire::timestamp;
use crate::dto::public::consignment::wire::uuid;
use crate::dto::public::consignment::wire_v2::command_uuid;
use crate::dto::public::consignment::wire_v2::reservation_code;
use crate::dto::public::consignment::write::write_object;

json_object! {
    (tag = "state", rename_all = "snake_case")
    /// 本人寄件指引。`none` 不携带任何秘密；`active` 才携带当前有效预约码。
    #[derive(Clone, PartialEq, Eq)]
    pub enum DepositInstructions {
        /// 当前没有可用的寄件预约。
        None {},
        /// 当前有效的寄件指引。
        Active {
            world: ListingWorld,
            #[serde(deserialize_with = "text::<128, _>")]
            bot: String,
            #[serde(deserialize_with = "text::<32, _>")]
            subject: String,
            #[serde(deserialize_with = "reservation_code")]
            body: String,
            #[serde(deserialize_with = "uuid")]
            reservation_id: Uuid,
            #[serde(deserialize_with = "timestamp")]
            expires_at: Timestamp,
        },
    } => validate
}

impl DepositInstructions {
    fn validate(&self) -> Result<(), &'static str> {
        if let Self::Active { bot, subject, body, reservation_id, .. } = self {
            if subject != "consign" {
                return Err("寄件主题必须为 consign");
            }
            if bot.is_empty() || bot.len() > 128 {
                return Err("寄件 Bot 名称必须为规范单行文本");
            }
            if reservation_id.is_nil() {
                return Err("预约标识不得为零 UUID");
            }
            // 不变量 7：预约码只经本人指引返回，不得进入调试输出或错误文本。
            if body.is_empty() {
                return Err("寄件预约码不得为空");
            }
        }
        Ok(())
    }
}

impl fmt::Debug for DepositInstructions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None {} => formatter.write_str("DepositInstructions::None"),
            Self::Active { world, bot, subject, reservation_id, expires_at, .. } => formatter
                .debug_struct("DepositInstructions::Active")
                .field("world", world)
                .field("bot", bot)
                .field("subject", subject)
                .field("body", &"[REDACTED]")
                .field("reservation_id", reservation_id)
                .field("expires_at", expires_at)
                .finish(),
        }
    }
}

write_object! {
    /// 本人签发或换发寄件预约。不变量 1、6：稳定 UUIDv4 与预期版本。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct IssueDepositReservation {
        pub command_id: Uuid => ("command_uuid", "必须为规范小写的非零 UUIDv4"),
        pub expected_version: ListingVersion => ("serde::Deserialize::deserialize", "必须为正的安全 JSON 整数"),
    }
}

json_object! {
    ()
    /// 预约签发成功的 201 回执。用户需要寄件码时另读本人指引。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct DepositReservationReceipt {
        /// 固定 v2。
        pub schema_version: u16,
        pub operation: ConsignmentOperationV2,
        pub disposition: ConsignmentDispositionV2,
        #[serde(deserialize_with = "uuid")]
        pub command_id: Uuid,
        #[serde(deserialize_with = "uuid")]
        pub listing_id: Uuid,
        pub listing_version: ListingVersion,
        pub state: ListingStateV2,
        #[serde(deserialize_with = "uuid")]
        pub reservation_id: Uuid,
        #[serde(deserialize_with = "timestamp")]
        pub expires_at: Timestamp,
    } => validate
}

impl DepositReservationReceipt {
    fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != 2
            || self.operation != ConsignmentOperationV2::IssueReservation
            || self.disposition != ConsignmentDispositionV2::ReservationIssued
            || self.state != ListingStateV2::AwaitingDeposit
        {
            return Err("预约回执的 operation、处置或状态不一致");
        }
        if self.command_id.is_nil() || self.listing_id.is_nil() || self.reservation_id.is_nil() {
            return Err("预约回执的标识不得为零 UUID");
        }
        Ok(())
    }
}
