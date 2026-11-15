//! 强类型标识与确定性 UUID 派生。

use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
/// namespace、命令 ID、业务标签和版本共同组成输入；标签或算法版本变化时必须显式升级
/// 版本，避免同一命令在不同业务事实之间复用 ID。该函数只负责确定性派生，不承担业务
/// 资格校验，调用方仍需在应用事务中保存对应命令账本。
pub fn deterministic_uuid(command_id: CommandId, label: &str, version: u32) -> Uuid {
    let name = format!("auction:{version}:{command_id}:{label}");
    Uuid::new_v5(&Uuid::NAMESPACE_OID, name.as_bytes())
}

/// 按已发布的长度前缀自然键算法派生稳定 UUID；字节兼容、不可修改，由
/// `crates/auction-domain/tests/identity_vectors.rs` 的冻结向量固定。
/// 该算法属于旧结算义务和 recovery 主键的持久兼容合同：namespace 与每个二进制部分都先写入
/// 八字节大端长度，再截取 SHA-256 前 16 字节并设置 UUID v5/variant 位。调用方必须继续使用
/// 已冻结的 namespace 和自然键顺序，不能改用可重试命令 ID 替代业务事实身份。
pub fn legacy_natural_key_uuid(namespace: &str, parts: &[&[u8]]) -> Uuid {
    let mut hash = Sha256::new();
    write_legacy_stable_part(&mut hash, namespace.as_bytes());
    for part in parts {
        write_legacy_stable_part(&mut hash, part);
    }
    legacy_v5_uuid(hash)
}

/// 按已发布的 Worker 稳定命令身份算法派生 UUID；字节兼容、不可修改，由
/// `crates/auction-domain/tests/identity_vectors.rs` 的 `worker_uuid_matches_frozen_vectors` 固定。
/// namespace、kind 与实体 UUID 依次按长度前缀写入，再追加 due 世代的八字节大端纳秒（负值按
/// 补码），截取方式与 [`legacy_natural_key_uuid`] 相同。结算工作流、领取/完成命令与慢速代理
/// 命令都按该算法持久化身份；调用方必须沿用已冻结的 namespace、kind 与 due 世代取值。
pub fn legacy_worker_uuid(namespace: &str, kind: &str, entity_id: Uuid, due_nanos: i64) -> Uuid {
    let mut hash = Sha256::new();
    for part in [namespace.as_bytes(), kind.as_bytes(), entity_id.as_bytes()] {
        write_legacy_stable_part(&mut hash, part);
    }
    hash.update(due_nanos.to_be_bytes());
    legacy_v5_uuid(hash)
}

fn write_legacy_stable_part(hash: &mut Sha256, value: &[u8]) {
    hash.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    hash.update(value);
}

/// 截取 SHA-256 前 16 字节并设置 UUID v5/variant 位；两种旧版稳定身份共用这一步。
fn legacy_v5_uuid(hash: Sha256) -> Uuid {
    let digest = hash.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
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
