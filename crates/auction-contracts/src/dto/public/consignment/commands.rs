//! 登记、改价、取消的输入及可重放的寄售命令回执。

use std::fmt;

use uuid::Uuid;

use crate::deserialize_required_option;
use crate::dto::public::consignment::scalar::ListingFeeBps;
use crate::dto::public::consignment::scalar::ListingPrice;
use crate::dto::public::consignment::scalar::ListingVersion;
use crate::dto::public::consignment::states::ListingMode;
use crate::dto::public::consignment::states::ListingState;
use crate::dto::public::consignment::wire::json_object;
use crate::dto::public::consignment::wire::optional_uuid;
use crate::dto::public::consignment::wire::string_enum;
use crate::dto::public::consignment::wire::uuid;
use crate::dto::public::consignment::write::write_object;

write_object! {
    /// 登记请求。不变量 1、6、7：稳定命令 ID、严格价格，装备链接只在输入中传递。
    #[derive(Clone, PartialEq, Eq)]
    pub struct RegisterListing {
        pub command_id: Uuid => ("uuid", "必须为规范小写的非零 UUID"),
        pub mode: ListingMode => ("serde::Deserialize::deserialize", "必须为约定的模式字符串"),
        pub equipment_url: String => ("serde::Deserialize::deserialize", "必须为字符串"),
        pub price: ListingPrice => ("serde::Deserialize::deserialize", "必须为范围内的 JSON 整数（1–1000000000）"),
        pub expected_fee_bps: ListingFeeBps => ("serde::Deserialize::deserialize", "必须为范围内的 JSON 整数（0–9999）"),
    }
}

impl fmt::Debug for RegisterListing {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RegisterListing")
            .field("command_id", &self.command_id)
            .field("mode", &self.mode)
            .field("equipment_url", &"[REDACTED]")
            .field("price", &self.price)
            .field("expected_fee_bps", &self.expected_fee_bps)
            .finish()
    }
}

write_object! {
    /// 改价请求。不变量 1、6：命令身份与预期版本必填，标价严格限界。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct RepriceListing {
        pub command_id: Uuid => ("uuid", "必须为规范小写的非零 UUID"),
        pub expected_version: ListingVersion => ("serde::Deserialize::deserialize", "必须为正的安全 JSON 整数"),
        pub price: ListingPrice => ("serde::Deserialize::deserialize", "必须为范围内的 JSON 整数（1–1000000000）"),
    }
}

write_object! {
    /// 取消请求。不变量 1、6：命令身份与预期版本必填。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct WithdrawListing {
        pub command_id: Uuid => ("uuid", "必须为规范小写的非零 UUID"),
        pub expected_version: ListingVersion => ("serde::Deserialize::deserialize", "必须为正的安全 JSON 整数"),
    }
}

string_enum! {
    /// 有 HTTP 回执的命令。解析提交与系统清退属于内部命令。
    pub enum ListingOperation {
        /// 玩家登记。
        Register => "register",
        /// 玩家改价。
        Reprice => "reprice",
        /// 玩家取消。
        Withdraw => "withdraw",
        /// Admin 下架。
        AdminDelist => "admin_delist",
    }
}

string_enum! {
    /// 命令初次提交时记录的处置。
    pub enum ListingDisposition {
        /// 登记已接受，装备等待解析。
        Registered => "registered",
        /// 标价已变更。
        Repriced => "repriced",
        /// 标价与原先相同。
        Unchanged => "unchanged",
        /// 挂单已关闭。
        Closed => "closed",
        /// 确定性业务拒绝，只在私有查命令结果中返回。
        Rejected => "rejected",
    }
}

// 写响应只能返回成功处置，账本查询可返回首次确定性拒绝。
macro_rules! command_receipt {
    ($(#[$attr:meta])* $name:ident, $allow_rejected:literal) => {
        json_object! {
            ()
            $(#[$attr])*
            #[derive(Debug, Clone, PartialEq, Eq)]
            pub struct $name {
                #[serde(deserialize_with = "uuid")]
                pub command_id: Uuid,
                pub operation: ListingOperation,
                #[serde(deserialize_with = "optional_uuid")]
                pub listing_id: Option<Uuid>,
                #[serde(deserialize_with = "deserialize_required_option")]
                pub listing_version: Option<ListingVersion>,
                #[serde(deserialize_with = "deserialize_required_option")]
                pub state: Option<ListingState>,
                pub disposition: ListingDisposition,
                #[serde(deserialize_with = "deserialize_required_option")]
                pub price: Option<ListingPrice>,
            } => validate
        }

        impl $name {
            fn validate(&self) -> Result<(), &'static str> {
                use ListingDisposition::{Closed, Registered, Rejected, Repriced, Unchanged};
                use ListingOperation::{AdminDelist, Register, Reprice, Withdraw};

                // 不变量 1、6、7：回执必须能定位成功目标，且处置与原操作相符。
                if self.disposition == Rejected && $allow_rejected {
                    return Ok(());
                }
                let valid_result = match (self.disposition, self.operation) {
                    (Registered, Register) => {
                        self.listing_version == ListingVersion::new(1)
                            && self.state == Some(ListingState::Resolving)
                            && self.price.is_some()
                    }
                    (Repriced | Unchanged, Reprice) => {
                        self.state.is_some_and(ListingState::is_active) && self.price.is_some()
                    }
                    (Closed, Withdraw | AdminDelist) => self.state == Some(ListingState::Closed),
                    _ => false,
                };
                if !valid_result || self.listing_id.is_none() || self.listing_version.is_none() {
                    return Err("寄售回执的操作、处置或目标字段不一致");
                }
                Ok(())
            }
        }
    };
}

command_receipt!(
    /// 写成功回执，始终携带可定位的目标。
    CommandReceipt,
    false
);
command_receipt!(
    /// 本人命令查询中的原始回执，确定性拒绝允许目标字段为空。
    StoredCommandReceipt,
    true
);

string_enum! {
    /// 已经仲裁并落账的结果码。解码、依赖故障、停用准入及命令 ID 冲突不占账本。
    ///
    /// 合同未给出独立的 result_code 表，这里只接受 §3、§7 给出且可落账的码。
    pub enum StoredCommandCode {
        /// 同步业务成功。
        Ok => "ok",
        /// 异步登记已接受。
        Accepted => "accepted",
        /// 参与者不具备资格。
        ParticipantIneligible => "participant_ineligible",
        /// 本人范围内无此目标。
        ListingNotFound => "listing_not_found",
        /// 同人同件已有活动单。
        AlreadyOpen => "already_open",
        /// 装备被占用。
        EquipmentBusy => "equipment_busy",
        /// 站内历史证据不足。
        HistoryUnproven => "history_unproven",
        /// 最后已证持有人不是登记人。
        HistoryOwnerMismatch => "history_owner_mismatch",
        /// 站内别名历史冲突。
        HistoryAliasConflict => "history_alias_conflict",
        /// 费率已变化。
        FeeChanged => "fee_changed",
        /// 请求版本已过期。
        StaleVersion => "stale_version",
        /// 挂单已停止接收动作。
        ListingNotActive => "listing_not_active",
        /// 当前世界不接受登记。
        WorldUnavailable => "world_unavailable",
        /// 超过滚动小时登记额度。
        RegistrationRateLimited => "registration_rate_limited",
        /// 超过活动挂单额度。
        ListingLimitReached => "listing_limit_reached",
    }
}

impl StoredCommandCode {
    fn http_status(self) -> i64 {
        match self {
            Self::Ok => 200,
            Self::Accepted => 202,
            Self::ParticipantIneligible => 403,
            Self::ListingNotFound => 404,
            Self::RegistrationRateLimited | Self::ListingLimitReached => 429,
            Self::AlreadyOpen
            | Self::EquipmentBusy
            | Self::HistoryUnproven
            | Self::HistoryOwnerMismatch
            | Self::HistoryAliasConflict
            | Self::FeeChanged
            | Self::StaleVersion
            | Self::ListingNotActive
            | Self::WorldUnavailable => 409,
        }
    }
}

json_object! {
    ()
    /// 查命令返回首次的 HTTP 状态与业务结果，外层 GET 的 200 与此无关。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct StoredCommandResult {
        pub http_status: i64,
        pub code: StoredCommandCode,
        pub receipt: StoredCommandReceipt,
    } => validate
}

impl StoredCommandResult {
    fn validate(&self) -> Result<(), &'static str> {
        let disposition_matches = match self.code {
            StoredCommandCode::Accepted => {
                self.receipt.disposition == ListingDisposition::Registered
            }
            StoredCommandCode::Ok => matches!(
                self.receipt.disposition,
                ListingDisposition::Repriced
                    | ListingDisposition::Unchanged
                    | ListingDisposition::Closed
            ),
            _ => self.receipt.disposition == ListingDisposition::Rejected,
        };
        if self.http_status != self.code.http_status() || !disposition_matches {
            return Err("已存寄售命令的状态码与回执处置不一致");
        }
        Ok(())
    }
}
