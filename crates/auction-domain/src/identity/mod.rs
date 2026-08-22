use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! typed_id {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        /// 具有独立类型语义的 UUID 标识。
        pub struct $name(Uuid);

        impl $name {
            /// 生成新的随机标识。
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// 从已有 UUID 建立领域标识。
            pub fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            /// 取出底层 UUID，供边界适配器转换。
            pub fn as_uuid(self) -> Uuid {
                self.0
            }

            /// 判断标识是否为 nil UUID。
            pub fn is_nil(self) -> bool {
                self.0.is_nil()
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self::from_uuid(value)
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.as_uuid()
            }
        }

        impl Display for $name {
            fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

/// 为可重放命令派生稳定 UUID。
///
/// namespace、命令 ID、业务标签和版本共同组成输入；标签或算法版本变化时必须显式升级
/// 版本，避免同一命令在不同业务事实之间复用 ID。该函数只负责确定性派生，不承担业务
/// 资格校验，调用方仍需在应用事务中保存对应命令账本。
pub fn deterministic_uuid(command_id: CommandId, label: &str, version: u32) -> Uuid {
    let name = format!("auction:{version}:{command_id}:{label}");
    Uuid::new_v5(&Uuid::NAMESPACE_OID, name.as_bytes())
}

typed_id!(AuctionId);
typed_id!(BidId);
typed_id!(CommandId);
typed_id!(EquipmentId);
typed_id!(LotId);
typed_id!(ProxyAuthorizationId);
typed_id!(SettlementId);
typed_id!(SubmissionId);
typed_id!(UserId);
typed_id!(WorldId);
