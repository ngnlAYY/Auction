//! Admin 损失案件读模型、灭失确认与人工结果的请求与回执。
//!
//! 案件与回执只有状态、版本、依据与私有说明：没有任何金额、付款或发行字段，也没有装备 key、
//! URL 或证据原文（不变量 6、7；决定 22）。

use auction_time::Timestamp;
use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;

use super::validate_delist_reason;
use crate::dto::public::consignment::wire::json_object;
use crate::dto::public::consignment::wire::text;
use crate::dto::public::consignment::wire::timestamp;
use crate::dto::public::consignment::wire::uuid;
use crate::dto::public::consignment::wire_v2::command_uuid;
use crate::dto::public::consignment::wire_v2::positive_version;
use crate::dto::public::consignment::wire_v2::required_option_uuid;
use crate::dto::public::consignment::wire_v2::serialize_positive_version;
use crate::dto::public::consignment::wire_v2::sha256_hex;
use crate::dto::public::consignment::wire_v2::string_enum;
use crate::dto::public::consignment::write::write_object;

/// 案件版本从 1 开始（版本 1 是开案记录），必须能由浏览器精确表示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LossCaseVersion(i64);

impl LossCaseVersion {
    /// 从已校验的安全整数构造。
    pub const fn new(value: i64) -> Option<Self> {
        if value >= 1 && value <= crate::dto::public::consignment::wire_v2::SAFE_INTEGER_MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// 返回案件版本整数。
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for LossCaseVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        positive_version(deserializer).map(Self)
    }
}

impl Serialize for LossCaseVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_positive_version(&self.0, serializer)
    }
}

string_enum! {
    /// 回执的固定操作名。
    pub enum LossCaseOperation {
        /// Admin 追加损失案件人工结果。
        RecordLossCaseOutcome => "record_loss_case_outcome",
    }
}

string_enum! {
    /// 回执的固定处置。
    pub enum LossCaseDisposition {
        /// 人工结果已追加。
        CaseOutcomeRecorded => "case_outcome_recorded",
    }
}

string_enum! {
    /// 案件状态；只读状态，不授予付款或发行能力。
    pub enum LossCaseState {
        /// 已开案，尚无人工结果。
        Opened => "opened",
        /// 复核中。
        Reviewing => "reviewing",
        /// 已办结。
        Closed => "closed",
    }
}

string_enum! {
    /// Admin 可追加的人工结果种类。
    pub enum LossCaseOutcomeKind {
        /// 复核中。
        Reviewing => "reviewing",
        /// 已办结。
        Closed => "closed",
    }
}

fn loss_note<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let note = String::deserialize(deserializer)?;
    validate_delist_reason(&note).map_err(serde::de::Error::custom)?;
    Ok(note)
}

fn optional_loss_note<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let note = Option::<String>::deserialize(deserializer)?;
    if let Some(note) = &note {
        validate_delist_reason(note).map_err(serde::de::Error::custom)?;
    }
    Ok(note)
}

fn serialize_loss_note<S: serde::Serializer>(note: &str, serializer: S) -> Result<S::Ok, S::Error> {
    validate_delist_reason(note).map_err(serde::ser::Error::custom)?;
    serializer.serialize_str(note)
}

write_object! {
    /// Admin 追加损失案件人工结果。不变量 1、6、7：稳定 UUIDv4、案件、预期案件版本、结果、依据与
    /// 私有说明；没有金额、付款或发行字段，多余字段按未知字段整体拒绝。
    #[derive(Clone, PartialEq, Eq)]
    pub struct RecordLossCaseOutcomeRequest {
        pub command_id: Uuid => ("command_uuid", "必须为规范小写的非零 UUIDv4"),
        pub case_id: Uuid => ("uuid", "必须为规范小写的非零 UUID"),
        pub expected_case_version: LossCaseVersion => ("serde::Deserialize::deserialize", "必须为正的安全 JSON 整数"),
        pub outcome: LossCaseOutcomeKind => ("serde::Deserialize::deserialize", "必须为 reviewing 或 closed"),
        pub basis_id: Uuid => ("uuid", "必须为规范小写的非零 UUID"),
        #[serde(serialize_with = "serialize_loss_note")]
        pub private_note: String => ("loss_note", "必须为不含装备链接或已知凭据形态的规范单行文本（1–256 字节）"),
    }
}

impl std::fmt::Debug for RecordLossCaseOutcomeRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RecordLossCaseOutcomeRequest")
            .field("command_id", &self.command_id)
            .field("case_id", &self.case_id)
            .field("expected_case_version", &self.expected_case_version)
            .field("outcome", &self.outcome)
            .field("basis_id", &self.basis_id)
            .field("private_note", &"[REDACTED]")
            .finish()
    }
}

json_object! {
    ()
    /// 人工结果的 200 回执：只有案件、版本与记录标识，没有金额、付款或发行字段。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct LossCaseOutcomeReceipt {
        /// 固定 v2。
        pub schema_version: u16,
        pub operation: LossCaseOperation,
        pub disposition: LossCaseDisposition,
        #[serde(deserialize_with = "uuid")]
        pub command_id: Uuid,
        #[serde(deserialize_with = "uuid")]
        pub case_id: Uuid,
        pub case_version: LossCaseVersion,
        #[serde(deserialize_with = "uuid")]
        pub outcome_id: Uuid,
        #[serde(deserialize_with = "uuid")]
        pub seller_receipt_id: Uuid,
    } => validate
}

impl LossCaseOutcomeReceipt {
    fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != 2 {
            return Err("损失案件结果回执的版本不一致");
        }
        // 版本 1 是开案记录；人工结果至少从版本 2 开始。
        if self.case_version.get() < 2 {
            return Err("人工结果回执的案件版本必须大于 1");
        }
        if self.command_id.is_nil()
            || self.case_id.is_nil()
            || self.outcome_id.is_nil()
            || self.seller_receipt_id.is_nil()
        {
            return Err("损失案件结果回执的标识不得为零 UUID");
        }
        Ok(())
    }
}

string_enum! {
    /// 灭失确认回执的固定操作名。
    pub enum PositionLossOperation {
        /// Admin 确认代管件灭失。
        ConfirmPositionLoss => "confirm_position_loss",
    }
}

string_enum! {
    /// 灭失确认回执的固定处置。
    pub enum PositionLossDisposition {
        /// 已按 position 办结。
        PositionLossRecorded => "position_loss_recorded",
    }
}

write_object! {
    /// Admin 确认代管件灭失。不变量 1、6、7：稳定 UUIDv4、预期 position 版本、独立证据 ID 与规范
    /// 私有说明；没有金额、退款或赔款字段，多余字段按未知字段整体拒绝。
    #[derive(Clone, PartialEq, Eq)]
    pub struct ConfirmPositionLossRequest {
        pub command_id: Uuid => ("command_uuid", "必须为规范小写的非零 UUIDv4"),
        pub expected_version: LossCaseVersion => ("serde::Deserialize::deserialize", "必须为正的安全 JSON 整数"),
        pub proof_id: Uuid => ("uuid", "必须为规范小写的非零 UUID"),
        #[serde(serialize_with = "serialize_loss_note")]
        pub reason: String => ("loss_note", "必须为不含装备链接或已知凭据形态的规范单行文本（1–256 字节）"),
    }
}

impl std::fmt::Debug for ConfirmPositionLossRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConfirmPositionLossRequest")
            .field("command_id", &self.command_id)
            .field("expected_version", &self.expected_version)
            .field("proof_id", &self.proof_id)
            .field("reason", &"[REDACTED]")
            .finish()
    }
}

json_object! {
    ()
    /// 灭失确认的 200 回执：挂单关闭、position 版本、损失事实、案件与卖家回执；没有合同、退款、
    /// 赔款或付款字段，也不回显私有说明（D9 §6.2）。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct PositionLossReceipt {
        /// 固定 v2。
        pub schema_version: u16,
        pub operation: PositionLossOperation,
        pub disposition: PositionLossDisposition,
        #[serde(deserialize_with = "uuid")]
        pub command_id: Uuid,
        #[serde(deserialize_with = "uuid")]
        pub listing_id: Uuid,
        pub listing_version: crate::ListingVersion,
        pub state: crate::ListingStateV2,
        #[serde(deserialize_with = "uuid")]
        pub position_id: Uuid,
        pub position_version: LossCaseVersion,
        #[serde(deserialize_with = "uuid")]
        pub loss_fact_id: Uuid,
        #[serde(deserialize_with = "uuid")]
        pub loss_case_id: Uuid,
        #[serde(deserialize_with = "uuid")]
        pub seller_receipt_id: Uuid,
    } => validate
}

impl PositionLossReceipt {
    fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != 2 || self.state != crate::ListingStateV2::Closed {
            return Err("灭失确认回执的版本或挂单状态不一致");
        }
        if [
            self.command_id,
            self.listing_id,
            self.position_id,
            self.loss_fact_id,
            self.loss_case_id,
            self.seller_receipt_id,
        ]
        .iter()
        .any(Uuid::is_nil)
        {
            return Err("灭失确认回执的标识不得为零 UUID");
        }
        Ok(())
    }
}

json_object! {
    ()
    /// 案件的一条追加记录。
    #[derive(Clone, PartialEq, Eq)]
    pub struct AdminLossOutcome {
        #[serde(deserialize_with = "uuid")]
        pub outcome_id: Uuid,
        pub case_version: LossCaseVersion,
        pub state: LossCaseState,
        #[serde(deserialize_with = "required_option_uuid")]
        pub basis_id: Option<Uuid>,
        #[serde(deserialize_with = "optional_loss_note", serialize_with = "serialize_optional_note")]
        pub private_note: Option<String>,
        #[serde(deserialize_with = "timestamp")]
        pub occurred_at: Timestamp,
        #[serde(deserialize_with = "uuid")]
        pub seller_receipt_id: Uuid,
    } => validate
}

fn serialize_optional_note<S: serde::Serializer>(
    note: &Option<String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match note {
        Some(note) => serialize_loss_note(note, serializer),
        None => serializer.serialize_none(),
    }
}

impl AdminLossOutcome {
    fn validate(&self) -> Result<(), &'static str> {
        if self.outcome_id.is_nil() || self.seller_receipt_id.is_nil() {
            return Err("案件记录的标识不得为零 UUID");
        }
        // 开案记录（版本 1）没有依据与说明，人工结果必须都有。
        let opening = self.case_version.get() == 1;
        let is_opened = self.state == LossCaseState::Opened;
        if opening != is_opened {
            return Err("只有版本 1 的案件记录可以是 opened");
        }
        if opening == (self.basis_id.is_some() || self.private_note.is_some()) {
            return Err("开案记录不得带依据或说明，人工结果必须带依据与说明");
        }
        Ok(())
    }
}

impl std::fmt::Debug for AdminLossOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminLossOutcome")
            .field("outcome_id", &self.outcome_id)
            .field("case_version", &self.case_version)
            .field("state", &self.state)
            .field("basis_id", &self.basis_id)
            .field("private_note", &self.private_note.as_ref().map(|_| "[REDACTED]"))
            .field("occurred_at", &self.occurred_at)
            .field("seller_receipt_id", &self.seller_receipt_id)
            .finish()
    }
}

json_object! {
    ()
    /// Admin 按 position 查到的损失案件；证据只给来源与摘要，不给原文，也没有任何金额字段。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct AdminLossCase {
        #[serde(deserialize_with = "uuid")]
        pub position_id: Uuid,
        #[serde(deserialize_with = "uuid")]
        pub case_id: Uuid,
        #[serde(deserialize_with = "uuid")]
        pub loss_fact_id: Uuid,
        #[serde(deserialize_with = "uuid")]
        pub proof_id: Uuid,
        #[serde(deserialize_with = "text::<64, _>")]
        pub proof_source_kind: String,
        #[serde(deserialize_with = "sha256_hex")]
        pub proof_digest: String,
        #[serde(deserialize_with = "uuid")]
        pub listing_id: Uuid,
        #[serde(deserialize_with = "uuid")]
        pub seller_user_id: Uuid,
        pub generation: LossCaseVersion,
        pub case_version: LossCaseVersion,
        pub state: LossCaseState,
        pub outcomes: Vec<AdminLossOutcome>,
    } => validate
}

impl AdminLossCase {
    fn validate(&self) -> Result<(), &'static str> {
        if [
            self.position_id,
            self.case_id,
            self.loss_fact_id,
            self.proof_id,
            self.listing_id,
            self.seller_user_id,
        ]
        .iter()
        .any(Uuid::is_nil)
        {
            return Err("损失案件的标识不得为零 UUID");
        }
        let versions_are_consecutive = self
            .outcomes
            .iter()
            .zip(1_i64..)
            .all(|(outcome, expected)| outcome.case_version.get() == expected);
        let Some(latest) = self.outcomes.last() else {
            return Err("损失案件至少有开案记录");
        };
        if !versions_are_consecutive
            || latest.case_version != self.case_version
            || latest.state != self.state
        {
            return Err("损失案件的版本与状态必须与最新记录一致");
        }
        Ok(())
    }
}

mod proof;

pub use proof::*;

#[cfg(test)]
mod tests;
