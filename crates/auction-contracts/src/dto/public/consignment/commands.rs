//! 登记、改价、取消的输入及可重放的寄售命令回执。

use std::fmt;

use serde::Deserialize;
use serde::Serialize;
use serde::de::Error;
use uuid::Uuid;

use crate::deserialize_required_option;
use crate::dto::public::consignment::states::ListingMode;
use crate::dto::public::consignment::states::ListingState;
use crate::dto::public::consignment::wire::FEE_BPS_MAX;
use crate::dto::public::consignment::wire::PRICE_MAX;
use crate::dto::public::consignment::wire::SAFE_INTEGER_MAX;
use crate::dto::public::consignment::wire::integer;
use crate::dto::public::consignment::wire::json_object;
use crate::dto::public::consignment::wire::optional_integer;
use crate::dto::public::consignment::wire::optional_uuid;
use crate::dto::public::consignment::wire::uuid;

json_object! {
    ()
    /// 登记请求。不变量 1、6、7：稳定命令 ID、严格价格，装备链接只在输入中传递。
    /// 写入口应将解码失败映射为安全错误码，不记录 Serde 原始错误或请求正文。
    #[derive(Clone, PartialEq, Eq)]
    pub struct RegisterListing {
        #[serde(deserialize_with = "uuid")]
        pub command_id: Uuid,
        pub mode: ListingMode,
        pub equipment_url: String,
        #[serde(deserialize_with = "integer::<1, PRICE_MAX, _>")]
        pub price: i64,
        #[serde(deserialize_with = "integer::<0, FEE_BPS_MAX, _>")]
        pub expected_fee_bps: i64,
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

json_object! {
    ()
    /// 改价请求。不变量 1、6：命令身份与预期版本必填，标价严格限界。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct RepriceListing {
        #[serde(deserialize_with = "uuid")]
        pub command_id: Uuid,
        #[serde(deserialize_with = "integer::<1, SAFE_INTEGER_MAX, _>")]
        pub expected_version: i64,
        #[serde(deserialize_with = "integer::<1, PRICE_MAX, _>")]
        pub price: i64,
    }
}

json_object! {
    ()
    /// 取消请求。不变量 1、6：命令身份与预期版本必填。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct WithdrawListing {
        #[serde(deserialize_with = "uuid")]
        pub command_id: Uuid,
        #[serde(deserialize_with = "integer::<1, SAFE_INTEGER_MAX, _>")]
        pub expected_version: i64,
    }
}

/// 有 HTTP 回执的命令。解析提交与系统清退属于内部命令。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ListingOperation {
    /// 玩家登记。
    Register,
    /// 玩家改价。
    Reprice,
    /// 玩家取消。
    Withdraw,
    /// Admin 下架。
    AdminDelist,
}

/// 命令初次提交时记录的处置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ListingDisposition {
    /// 登记已接受，装备等待解析。
    Registered,
    /// 标价已变更。
    Repriced,
    /// 标价与原先相同。
    Unchanged,
    /// 挂单已关闭。
    Closed,
    /// 确定性业务拒绝，只在私有查命令结果中返回。
    Rejected,
}

json_object! {
    ()
    /// 不变量 1、7：重放初次的安全业务回执，凭证与外部原文不进入回执。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct CommandReceipt {
        #[serde(deserialize_with = "uuid")]
        pub command_id: Uuid,
        pub operation: ListingOperation,
        #[serde(deserialize_with = "optional_uuid")]
        pub listing_id: Option<Uuid>,
        #[serde(deserialize_with = "optional_integer::<1, SAFE_INTEGER_MAX, _>")]
        pub listing_version: Option<i64>,
        #[serde(deserialize_with = "deserialize_required_option")]
        pub state: Option<ListingState>,
        pub disposition: ListingDisposition,
        #[serde(deserialize_with = "optional_integer::<1, PRICE_MAX, _>")]
        pub price: Option<i64>,
    }
}

/// 已经仲裁并落账的结果码。解码、依赖故障、停用准入及命令 ID 冲突不占账本。
///
/// 合同未给出独立的 result_code 表，这里只接受 §3、§7 给出且可落账的码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StoredCommandCode {
    /// 同步业务成功。
    Ok,
    /// 异步登记已接受。
    Accepted,
    /// 参与者不具备资格。
    ParticipantIneligible,
    /// 本人范围内无此目标。
    ListingNotFound,
    /// 同人同件已有活动单。
    AlreadyOpen,
    /// 装备被占用。
    EquipmentBusy,
    /// 站内历史证据不足。
    HistoryUnproven,
    /// 最后已证持有人不是登记人。
    HistoryOwnerMismatch,
    /// 站内别名历史冲突。
    HistoryAliasConflict,
    /// 费率已变化。
    FeeChanged,
    /// 请求版本已过期。
    StaleVersion,
    /// 挂单已停止接收动作。
    ListingNotActive,
    /// 当前世界不接受登记。
    WorldUnavailable,
    /// 超过滚动小时登记额度。
    RegistrationRateLimited,
    /// 超过活动挂单额度。
    ListingLimitReached,
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
        pub receipt: CommandReceipt,
    } => validate
}

impl StoredCommandResult {
    fn validate<E: Error>(&self) -> Result<(), E> {
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
            return Err(E::custom("已存寄售命令的状态码与回执处置不一致"));
        }
        Ok(())
    }
}
