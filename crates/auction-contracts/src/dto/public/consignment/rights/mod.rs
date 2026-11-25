//! 受限权利的 Public HTTP 契约：找回、挑战、核验、当前权利与续办。
//!
//! 权利人没有登录会话；请求与响应都不携带会话令牌，令牌只在 `hv_right` Cookie 里。真实、虚拟与
//! 未匹配目标共用同一份受理响应形状（不变量 7）。

mod validation;

use auction_time::Timestamp;
use uuid::Uuid;

use crate::dto::public::consignment::states::ListingWorld;
use crate::dto::public::consignment::wire::json_object;
use crate::dto::public::consignment::wire::string_enum;
use crate::dto::public::consignment::wire::timestamp;
use crate::dto::public::consignment::wire::uuid;
use crate::dto::public::consignment::wire_v2::command_uuid;
use crate::dto::public::consignment::wire_v2::positive_version;
use crate::dto::public::consignment::wire_v2::serialize_positive_version;
use crate::dto::public::consignment::write::write_object;
use validation::challenge_code_input;
use validation::game_identity_text;
use validation::right_locator_text;

string_enum! {
    /// 受限会话可请求的动作闭集。
    pub enum RightActionV2 {
        /// 续办订单。
        ContinueOrder => "continue_order",
        /// 领取装备。
        ClaimEquipment => "claim_equipment",
        /// 领取款项。
        ClaimPayment => "claim_payment",
        /// 取回代管件。
        WithdrawHeld => "withdraw_held",
    }
}

string_enum! {
    /// 权利种类；损失案件只读。
    pub enum RightKindV2 {
        /// 续办订单。
        ContinueOrder => "continue_order",
        /// 领取装备。
        ClaimEquipment => "claim_equipment",
        /// 领取款项。
        ClaimPayment => "claim_payment",
        /// 取回代管件。
        WithdrawHeld => "withdraw_held",
        /// 损失案件，没有动作。
        LossCase => "loss_case",
    }
}

string_enum! {
    /// 续办处置。
    pub enum RightClaimDispositionV2 {
        /// 本命令受理。
        ClaimAccepted => "claim_accepted",
        /// 另一命令已在履约。
        AlreadyFulfilling => "already_fulfilling",
        /// 已完成。
        AlreadyCompleted => "already_completed",
    }
}

impl RightKindV2 {
    /// 该种类对应的唯一动作；损失案件没有。
    pub const fn action(self) -> Option<RightActionV2> {
        match self {
            Self::ContinueOrder => Some(RightActionV2::ContinueOrder),
            Self::ClaimEquipment => Some(RightActionV2::ClaimEquipment),
            Self::ClaimPayment => Some(RightActionV2::ClaimPayment),
            Self::WithdrawHeld => Some(RightActionV2::WithdrawHeld),
            Self::LossCase => None,
        }
    }
}

write_object! {
    /// 按游戏身份找回定位符指引。不变量 1、6：稳定 UUIDv4 与严格角色名。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct LocateRightRequest {
        pub command_id: Uuid => ("command_uuid", "必须为规范小写的非零 UUIDv4"),
        pub game_identity: String => ("game_identity_text", "必须为规范游戏角色名"),
        pub world: ListingWorld => ("serde::Deserialize::deserialize", "必须为已知世界"),
    }
}

write_object! {
    /// 按权利定位符请求挑战码。定位符是秘密，不进 Debug 之外的任何输出。
    #[derive(Clone, PartialEq, Eq)]
    pub struct ChallengeRightRequest {
        pub command_id: Uuid => ("command_uuid", "必须为规范小写的非零 UUIDv4"),
        pub right_locator: String => ("right_locator_text", "必须为 R1- 加 43 个 base64url 字符"),
    }
}

impl std::fmt::Debug for ChallengeRightRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChallengeRightRequest")
            .field("command_id", &self.command_id)
            .field("right_locator", &"[REDACTED]")
            .finish()
    }
}

write_object! {
    /// 用挑战码换取单权利会话。码是秘密，`Debug` 打码。
    #[derive(Clone, PartialEq, Eq)]
    pub struct VerifyRightRequest {
        pub command_id: Uuid => ("command_uuid", "必须为规范小写的非零 UUIDv4"),
        pub challenge_id: Uuid => ("command_uuid", "必须为规范小写的非零 UUIDv4"),
        pub code: String => ("challenge_code_input", "必须为不超长的字母数字与连字符"),
    }
}

impl std::fmt::Debug for VerifyRightRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VerifyRightRequest")
            .field("command_id", &self.command_id)
            .field("challenge_id", &self.challenge_id)
            .field("code", &"[REDACTED]")
            .finish()
    }
}

write_object! {
    /// 以当前会话办理权利。不变量 1、2：稳定 UUIDv4、已读到的权利版本与闭集动作。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ClaimRightRequest {
        pub command_id: Uuid => ("command_uuid", "必须为规范小写的非零 UUIDv4"),
        pub expected_version: i64 => ("positive_version", "必须为正的安全 JSON 整数"),
        pub action: RightActionV2 => ("serde::Deserialize::deserialize", "必须为已知动作"),
    }
}

json_object! {
    ()
    /// 找回与挑战的同形 202 受理结果：真实、虚拟与未匹配目标字节一致。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct RightChallengeAccepted {
        #[serde(deserialize_with = "command_uuid")]
        pub challenge_id: Uuid,
        #[serde(deserialize_with = "timestamp")]
        pub expires_at: Timestamp,
    }
}

json_object! {
    ()
    /// 核验成功的 200 结果；会话令牌只在 `hv_right` Cookie 里，不在正文。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct RightVerifiedGrant {
        #[serde(deserialize_with = "uuid")]
        pub right_id: Uuid,
        #[serde(deserialize_with = "positive_version", serialize_with = "serialize_positive_version")]
        pub right_version: i64,
        #[serde(deserialize_with = "timestamp")]
        pub expires_at: Timestamp,
    }
}

json_object! {
    (tag = "kind", rename_all = "snake_case")
    /// 权利指向的合同或代管件。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum RightTargetV2 {
        /// 寄售合同。
        Contract {
            #[serde(deserialize_with = "uuid")]
            contract_id: Uuid
        },
        /// 代管 position。
        Position {
            #[serde(deserialize_with = "uuid")]
            position_id: Uuid
        },
    }
}

json_object! {
    ()
    /// 当前会话对应的权利视图；不含定位符、挑战码或会话令牌。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct RightViewV2 {
        #[serde(deserialize_with = "uuid")]
        pub right_id: Uuid,
        #[serde(deserialize_with = "positive_version", serialize_with = "serialize_positive_version")]
        pub right_version: i64,
        pub kind: RightKindV2,
        pub world: ListingWorld,
        pub target: RightTargetV2,
        pub allowed_actions: Vec<RightActionV2>,
    } => validate
}

impl RightViewV2 {
    fn validate(&self) -> Result<(), &'static str> {
        // 不变量 6：动作只能是本种类的唯一动作，损失案件没有任何动作。
        let allowed = self.kind.action();
        if self.allowed_actions.iter().any(|action| Some(*action) != allowed)
            || self.allowed_actions.len() > 1
        {
            return Err("权利动作与权利种类不一致");
        }
        Ok(())
    }
}

json_object! {
    ()
    /// 续办命令的受理与回执：同一命令 ID 重放得到同一份。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct RightClaimReceipt {
        #[serde(deserialize_with = "command_uuid")]
        pub command_id: Uuid,
        #[serde(deserialize_with = "uuid")]
        pub right_id: Uuid,
        pub disposition: RightClaimDispositionV2,
    }
}
