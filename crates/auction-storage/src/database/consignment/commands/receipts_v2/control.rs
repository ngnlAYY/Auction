//! 起写、邮件证明、撤回与运营止售回执的账本映射。

use auction_application::consignment::{
    CustodyListingChange, DEPOSIT_SCHEMA_VERSION, DepositOperation, ListingStateChange,
    OperationsHoldDisposition, OperationsHoldReceipt, ProofReceipt, ReceiptDisposition,
    StartReceipt, WithdrawDepositReceipt,
};
use auction_domain::CommandId;
use auction_domain::consignment::HeldOffMarketReason;
use auction_domain::consignment::deposit::PositionState;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::DepositReceipt;
use super::common;
use super::common_body;
use super::corrupt;
use super::corrupt_from;
use super::decode_listing_changes;
use super::finish_body;
use super::listing_changes;
use super::require_status;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::ledger::StoredReceipt;

impl DepositReceipt for StartReceipt {
    fn operation(&self) -> DepositOperation {
        DepositOperation::StartDispositionWrite
    }

    fn status(&self) -> i16 {
        200
    }

    fn disposition(&self) -> ReceiptDisposition {
        ReceiptDisposition::WriteStarted
    }

    fn command_id(&self) -> CommandId {
        self.command_id
    }

    fn body(&self) -> Value {
        let mut body = common(self.operation(), self.disposition(), self.command_id);
        body.insert("decision_id".into(), json!(self.decision_id.to_string()));
        body.insert("work_id".into(), json!(self.work_id.to_string()));
        body.insert("attempt_id".into(), json!(self.attempt_id.to_string()));
        body.insert("fence".into(), json!(self.fence));
        finish_body(body)
    }

    fn decode(stored: &StoredReceipt) -> Result<Self, WriteError> {
        #[derive(Deserialize)]
        struct Body {
            decision_id: Uuid,
            work_id: Uuid,
            attempt_id: Uuid,
            fence: u64,
        }
        let common = common_body(stored, DepositOperation::StartDispositionWrite)?;
        let body: Body = serde_json::from_value(stored.body.clone()).map_err(corrupt_from)?;
        require_status(
            stored,
            &common,
            Self::started(
                CommandId::from_uuid(common.command_id),
                body.decision_id,
                body.work_id,
                body.attempt_id,
                body.fence,
            ),
        )
    }
}

fn custody_change(change: &CustodyListingChange) -> Value {
    json!({
        "listing_id": change.listing_id.to_string(),
        "listing_version": change.listing_version,
        "state": change.state.as_str(),
        "held_reason": change.held_reason.map(HeldOffMarketReason::as_str),
        "position_id": change.position_id.to_string(),
        "position_state": change.position_state.as_str(),
    })
}

impl DepositReceipt for ProofReceipt {
    fn operation(&self) -> DepositOperation {
        DepositOperation::ApplyMailProof
    }

    fn status(&self) -> i16 {
        200
    }

    fn disposition(&self) -> ReceiptDisposition {
        self.disposition
    }

    fn command_id(&self) -> CommandId {
        self.command_id
    }

    fn body(&self) -> Value {
        let mut body = common(self.operation(), self.disposition, self.command_id);
        body.insert("decision_id".into(), json!(self.decision_id.to_string()));
        body.insert("proof_id".into(), json!(self.proof_id.to_string()));
        body.insert("listing_changes".into(), listing_changes(&self.listing_changes));
        body.insert(
            "custody_changes".into(),
            Value::Array(self.custody_changes.iter().map(custody_change).collect()),
        );
        finish_body(body)
    }

    fn listing_changes(&self) -> Vec<ListingStateChange> {
        self.listing_changes.clone()
    }

    fn decode(stored: &StoredReceipt) -> Result<Self, WriteError> {
        #[derive(Deserialize)]
        struct Body {
            decision_id: Uuid,
            proof_id: Uuid,
            listing_changes: Option<Value>,
            custody_changes: Option<Value>,
        }
        #[derive(Deserialize)]
        struct Custody {
            listing_id: Uuid,
            listing_version: u64,
            state: String,
            held_reason: Option<String>,
            position_id: Uuid,
            position_state: String,
        }
        let common = common_body(stored, DepositOperation::ApplyMailProof)?;
        let body: Body = serde_json::from_value(stored.body.clone()).map_err(corrupt_from)?;
        let disposition = ReceiptDisposition::ALL
            .into_iter()
            .find(|disposition| disposition.as_str() == common.disposition)
            .ok_or_else(corrupt)?;
        let custody_changes = match body.custody_changes {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items
                .into_iter()
                .map(|item| {
                    let custody: Custody = serde_json::from_value(item).map_err(corrupt_from)?;
                    Ok(CustodyListingChange {
                        listing_id: custody.listing_id,
                        listing_version: custody.listing_version,
                        state: custody.state.parse().map_err(corrupt_from)?,
                        held_reason: custody
                            .held_reason
                            .as_deref()
                            .map(HeldOffMarketReason::parse)
                            .transpose()
                            .map_err(corrupt_from)?,
                        position_id: custody.position_id,
                        position_state: PositionState::parse(&custody.position_state)
                            .ok_or_else(corrupt)?,
                    })
                })
                .collect::<Result<Vec<_>, WriteError>>()?,
            Some(_) => return Err(corrupt()),
        };
        require_status(
            stored,
            &common,
            Self {
                schema_version: DEPOSIT_SCHEMA_VERSION,
                operation: DepositOperation::ApplyMailProof,
                disposition,
                command_id: CommandId::from_uuid(common.command_id),
                decision_id: body.decision_id,
                proof_id: body.proof_id,
                listing_changes: decode_listing_changes(body.listing_changes)?,
                custody_changes,
            },
        )
    }
}

impl DepositReceipt for WithdrawDepositReceipt {
    fn operation(&self) -> DepositOperation {
        DepositOperation::Withdraw
    }

    fn status(&self) -> i16 {
        if self.disposition == ReceiptDisposition::Closed { 200 } else { 202 }
    }

    fn disposition(&self) -> ReceiptDisposition {
        self.disposition
    }

    fn command_id(&self) -> CommandId {
        self.command_id
    }

    fn body(&self) -> Value {
        let mut body = common(self.operation(), self.disposition, self.command_id);
        body.insert("listing_id".into(), json!(self.listing_id.to_string()));
        body.insert("listing_version".into(), json!(self.listing_version.value()));
        body.insert("state".into(), json!(self.state.as_str()));
        if let Some(intent_id) = self.intent_id {
            body.insert("intent_id".into(), json!(intent_id.to_string()));
        }
        finish_body(body)
    }

    fn listing_changes(&self) -> Vec<ListingStateChange> {
        vec![ListingStateChange {
            listing_id: self.listing_id,
            listing_version: self.listing_version.value(),
            state: self.state,
        }]
    }

    fn decode(stored: &StoredReceipt) -> Result<Self, WriteError> {
        #[derive(Deserialize)]
        struct Body {
            listing_id: Uuid,
            listing_version: u64,
            state: String,
            intent_id: Option<Uuid>,
        }
        let common = common_body(stored, DepositOperation::Withdraw)?;
        let body: Body = serde_json::from_value(stored.body.clone()).map_err(corrupt_from)?;
        let disposition = ReceiptDisposition::ALL
            .into_iter()
            .find(|disposition| disposition.as_str() == common.disposition)
            .ok_or_else(corrupt)?;
        require_status(
            stored,
            &common,
            Self {
                schema_version: DEPOSIT_SCHEMA_VERSION,
                operation: DepositOperation::Withdraw,
                disposition,
                command_id: CommandId::from_uuid(common.command_id),
                listing_id: body.listing_id,
                listing_version: auction_domain::consignment::ListingVersion::new(
                    body.listing_version,
                )
                .map_err(corrupt_from)?,
                state: body.state.parse().map_err(corrupt_from)?,
                intent_id: body.intent_id,
                contract_id: None,
                equipment_id: None,
            },
        )
    }
}

impl DepositReceipt for OperationsHoldReceipt {
    fn operation(&self) -> DepositOperation {
        self.operation
    }

    fn status(&self) -> i16 {
        200
    }

    fn disposition(&self) -> ReceiptDisposition {
        self.disposition
    }

    fn command_id(&self) -> CommandId {
        self.command_id
    }

    fn body(&self) -> Value {
        let mut body = common(self.operation, self.disposition, self.command_id);
        body.insert("listing_id".into(), json!(self.listing_id.to_string()));
        body.insert("listing_version".into(), json!(self.listing_version.value()));
        body.insert("state".into(), json!(self.state.as_str()));
        // 不变量 1、6：`closed` 不带 hold；另两支的 hold 已由 4A 构造函数保证存在。
        if self.disposition != ReceiptDisposition::Closed
            && let Some(hold_id) = self.hold_id
        {
            body.insert("hold_id".into(), json!(hold_id.to_string()));
        }
        body.insert(
            "held_reason".into(),
            self.held_reason.map_or(Value::Null, |reason| json!(reason.as_str())),
        );
        finish_body(body)
    }

    fn listing_changes(&self) -> Vec<ListingStateChange> {
        vec![ListingStateChange {
            listing_id: self.listing_id,
            listing_version: self.listing_version.value(),
            state: self.state,
        }]
    }

    fn decode(stored: &StoredReceipt) -> Result<Self, WriteError> {
        #[derive(Deserialize)]
        struct Body {
            listing_id: Uuid,
            listing_version: u64,
            state: String,
            #[serde(default)]
            hold_id: Option<Uuid>,
            #[serde(default)]
            held_reason: Option<String>,
        }
        // 止售回执由两种操作写入：先按账本里的 operation 显式分派，再按该操作解码公共字段。
        let operation = match stored.body.get("operation").and_then(Value::as_str) {
            Some(code) if code == DepositOperation::AdminDelist.as_str() => {
                DepositOperation::AdminDelist
            }
            Some(code) if code == DepositOperation::ReleaseOperationsHold.as_str() => {
                DepositOperation::ReleaseOperationsHold
            }
            _ => return Err(corrupt()),
        };
        let common = common_body(stored, operation)?;
        let body: Body = serde_json::from_value(stored.body.clone()).map_err(corrupt_from)?;
        let disposition = match common.disposition.as_str() {
            "closed" => OperationsHoldDisposition::Closed,
            "hold_set" => OperationsHoldDisposition::HoldSet,
            "hold_released" => OperationsHoldDisposition::HoldReleased,
            _ => return Err(corrupt()),
        };
        // 反向解码同样经 4A 构造函数：`closed` 必无 hold，另两支必有 hold，
        // 且操作、状态与处置的固定组合不符时失败关闭。
        let receipt = Self::new(
            CommandId::from_uuid(common.command_id),
            disposition,
            body.listing_id,
            auction_domain::consignment::ListingVersion::new(body.listing_version)
                .map_err(corrupt_from)?,
            body.state.parse().map_err(corrupt_from)?,
            body.hold_id,
            body.held_reason
                .as_deref()
                .map(HeldOffMarketReason::parse)
                .transpose()
                .map_err(corrupt_from)?,
        )
        .map_err(corrupt_from)?;
        if receipt.operation.as_str() != common.operation {
            return Err(corrupt());
        }
        require_status(stored, &common, receipt)
    }
}
