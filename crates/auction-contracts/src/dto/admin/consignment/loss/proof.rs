//! Admin 录入灭失证据的请求与回执（rulings Q4）。
//!
//! 请求只有来源、观察身份、最后确定位置与私有说明：三个摘要由存储在锁内推导，不收自请求；观察身份
//! 与最后确定位置拒装备链接与 key 形状。回执只有 position、证据 ID 与摘要，不回显私有说明（不变量 6、7）。

use serde::Deserialize;
use uuid::Uuid;

use crate::dto::admin::consignment::validate_delist_reason;
use crate::dto::public::consignment::wire::checked_text;
use crate::dto::public::consignment::wire::json_object;
use crate::dto::public::consignment::wire::text;
use crate::dto::public::consignment::wire::uuid;
use crate::dto::public::consignment::wire_v2::command_uuid;
use crate::dto::public::consignment::wire_v2::sha256_hex;
use crate::dto::public::consignment::wire_v2::string_enum;
use crate::dto::public::consignment::write::write_object;

/// 观察身份：1–128 字节规范单行文本，并与私有说明同一形状筛查（与存储的证据充分性检查一致）。
fn observation_text<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    checked_text::<128>(&value).map_err(serde::de::Error::custom)?;
    validate_delist_reason(&value).map_err(serde::de::Error::custom)?;
    Ok(value)
}

write_object! {
    /// Admin 录入一条灭失证据。不变量 1、6、7：稳定 UUIDv4、来源、观察身份、最后确定位置与私有说明；
    /// 没有摘要、金额或任何派生字段，多余字段按未知字段整体拒绝。
    #[derive(Clone, PartialEq, Eq)]
    pub struct RecordLossProofRequest {
        pub command_id: Uuid => ("command_uuid", "必须为规范小写的非零 UUIDv4"),
        pub source_kind: String => ("text::<64, _>", "必须为 1–64 字节的规范单行文本"),
        pub observation_ref: String => ("observation_text", "必须为不含装备链接或已知凭据形态的规范单行文本（1–128 字节）"),
        #[serde(serialize_with = "super::serialize_loss_note")]
        pub last_known_ref: String => ("super::loss_note", "必须为不含装备链接或已知凭据形态的规范单行文本（1–256 字节）"),
        #[serde(serialize_with = "super::serialize_loss_note")]
        pub private_note: String => ("super::loss_note", "必须为不含装备链接或已知凭据形态的规范单行文本（1–256 字节）"),
    }
}

impl std::fmt::Debug for RecordLossProofRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RecordLossProofRequest")
            .field("command_id", &self.command_id)
            .field("source_kind", &self.source_kind)
            .field("observation_ref", &"[REDACTED]")
            .field("last_known_ref", &"[REDACTED]")
            .field("private_note", &"[REDACTED]")
            .finish()
    }
}

string_enum! {
    /// 灭失证据回执的固定操作名。
    pub enum LossProofOperation {
        /// Admin 录入灭失证据。
        RecordPositionLossProof => "record_position_loss_proof",
    }
}

string_enum! {
    /// 灭失证据回执的固定处置。
    pub enum LossProofDisposition {
        /// 证据已追加（或同来源同观察的原证据）。
        LossProofRecorded => "loss_proof_recorded",
    }
}

json_object! {
    ()
    /// 灭失证据的 200 回执：position、证据 ID 与证据摘要；不回显私有说明与证据原文（D9 §6.2）。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct LossProofReceipt {
        /// 固定 v2。
        pub schema_version: u16,
        pub operation: LossProofOperation,
        pub disposition: LossProofDisposition,
        #[serde(deserialize_with = "uuid")]
        pub command_id: Uuid,
        #[serde(deserialize_with = "uuid")]
        pub position_id: Uuid,
        #[serde(deserialize_with = "uuid")]
        pub proof_id: Uuid,
        #[serde(deserialize_with = "sha256_hex")]
        pub proof_digest: String,
    } => validate
}

impl LossProofReceipt {
    fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != 2 {
            return Err("灭失证据回执的版本不一致");
        }
        if [self.command_id, self.position_id, self.proof_id].iter().any(Uuid::is_nil) {
            return Err("灭失证据回执的标识不得为零 UUID");
        }
        Ok(())
    }
}
