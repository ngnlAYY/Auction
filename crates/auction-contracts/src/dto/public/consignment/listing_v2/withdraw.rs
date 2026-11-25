//! 本人 v2 撤回的请求与三种合法回执。

use uuid::Uuid;

use crate::dto::public::consignment::listing_v2::ListingStateV2;
use crate::dto::public::consignment::receipt::ConsignmentOperationV2;
use crate::dto::public::consignment::scalar::ListingVersion;
use crate::dto::public::consignment::wire::json_object;
use crate::dto::public::consignment::wire::uuid;
use crate::dto::public::consignment::wire_v2::command_uuid;
use crate::dto::public::consignment::write::write_object;

json_object! {
    (tag = "disposition", rename_all = "snake_case")
    /// 本人 v2 撤回的三种合法回执：代管件登记意愿、已在库件直接建撤回代、未代管件按前三期关闭。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum WithdrawReceiptV2 {
        /// 代管件登记撤回意愿。
        WithdrawalRequested {
            /// 固定 v2。
            schema_version: u16,
            operation: ConsignmentOperationV2,
            #[serde(deserialize_with = "uuid")]
            command_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            listing_id: Uuid,
            listing_version: ListingVersion,
            state: ListingStateV2,
            #[serde(deserialize_with = "uuid")]
            intent_id: Uuid,
        },
        /// 已在库件直接绑定撤回合同（7C-2）：合同与装备已确定，挂单进入 `withdrawal_pending`。
        WithdrawalAccepted {
            /// 固定 v2。
            schema_version: u16,
            operation: ConsignmentOperationV2,
            #[serde(deserialize_with = "uuid")]
            command_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            listing_id: Uuid,
            listing_version: ListingVersion,
            state: ListingStateV2,
            #[serde(deserialize_with = "uuid")]
            contract_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            equipment_id: Uuid,
        },
        /// 未代管件直接关闭。
        Closed {
            /// 固定 v2。
            schema_version: u16,
            operation: ConsignmentOperationV2,
            #[serde(deserialize_with = "uuid")]
            command_id: Uuid,
            #[serde(deserialize_with = "uuid")]
            listing_id: Uuid,
            listing_version: ListingVersion,
            state: ListingStateV2,
        },
    } => validate
}

write_object! {
    /// 本人 v2 撤回或取消请求。不变量 1、6：稳定 UUIDv4 与预期版本。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct WithdrawListingV2 {
        pub command_id: Uuid => ("command_uuid", "必须为规范小写的非零 UUIDv4"),
        pub expected_version: ListingVersion => ("serde::Deserialize::deserialize", "必须为正的安全 JSON 整数"),
    }
}

impl WithdrawReceiptV2 {
    fn validate(&self) -> Result<(), &'static str> {
        // 代管件登记意愿时：available 转入 withdrawal_pending，deposit_accepted/held_off_market
        // 保留原态；直接建撤回代时一律 withdrawal_pending（D9 §5.3 第 2–3 步）。
        match self {
            Self::WithdrawalRequested {
                schema_version,
                operation,
                command_id,
                listing_id,
                state,
                intent_id,
                ..
            } => {
                if *schema_version != 2
                    || *operation != ConsignmentOperationV2::Withdraw
                    || !matches!(
                        state,
                        ListingStateV2::DepositAccepted
                            | ListingStateV2::WithdrawalPending
                            | ListingStateV2::HeldOffMarket
                    )
                {
                    return Err("撤回意愿回执的版本、operation 或状态不一致");
                }
                if command_id.is_nil() || listing_id.is_nil() || intent_id.is_nil() {
                    return Err("撤回意愿回执的标识不得为零 UUID");
                }
            }
            Self::WithdrawalAccepted {
                schema_version,
                operation,
                command_id,
                listing_id,
                state,
                contract_id,
                equipment_id,
                ..
            } => {
                if *schema_version != 2
                    || *operation != ConsignmentOperationV2::Withdraw
                    || *state != ListingStateV2::WithdrawalPending
                {
                    return Err("撤回受理回执的版本、operation 或状态不一致");
                }
                if command_id.is_nil()
                    || listing_id.is_nil()
                    || contract_id.is_nil()
                    || equipment_id.is_nil()
                {
                    return Err("撤回受理回执的标识不得为零 UUID");
                }
            }
            Self::Closed { schema_version, operation, command_id, listing_id, state, .. } => {
                if *schema_version != 2
                    || *operation != ConsignmentOperationV2::Withdraw
                    || *state != ListingStateV2::Closed
                {
                    return Err("未代管撤回回执的版本、operation 或状态不一致");
                }
                if command_id.is_nil() || listing_id.is_nil() {
                    return Err("未代管撤回回执的标识不得为零 UUID");
                }
            }
        }
        Ok(())
    }
}
