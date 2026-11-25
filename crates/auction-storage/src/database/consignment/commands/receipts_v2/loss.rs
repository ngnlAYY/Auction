//! 灭失证据、灭失确认与案件人工结果回执的账本映射。
//!
//! 字段名与 `000137` 的 `c4_valid_v2_receipt` 逐字一致：三种回执都不含合同、退款、赔款、付款或发行字段
//! （D9 §5.3，决定 22）；解码时任何形状偏差都按账本损坏失败关闭。

use auction_application::consignment::{
    DepositOperation, ListingStateChange, LossCaseOutcomeReceipt, LossProofReceipt,
    PositionLossReceipt, ReceiptDisposition,
};
use auction_application::core::lower_hex;
use auction_domain::CommandId;
use auction_domain::consignment::{ListingState, ListingVersion};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::DepositReceipt;
use super::common;
use super::common_body;
use super::corrupt;
use super::corrupt_from;
use super::finish_body;
use super::parse_digest;
use super::require_status;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::ledger::StoredReceipt;

impl DepositReceipt for LossProofReceipt {
    fn operation(&self) -> DepositOperation {
        DepositOperation::RecordPositionLossProof
    }

    fn status(&self) -> i16 {
        200
    }

    fn disposition(&self) -> ReceiptDisposition {
        ReceiptDisposition::LossProofRecorded
    }

    fn command_id(&self) -> CommandId {
        self.command_id
    }

    fn body(&self) -> Value {
        let mut body = common(self.operation(), self.disposition(), self.command_id);
        body.insert("position_id".into(), json!(self.position_id.to_string()));
        body.insert("proof_id".into(), json!(self.proof_id.to_string()));
        body.insert("proof_digest".into(), json!(lower_hex(&self.proof_digest)));
        finish_body(body)
    }

    fn decode(stored: &StoredReceipt) -> Result<Self, WriteError> {
        #[derive(Deserialize)]
        struct Body {
            position_id: Uuid,
            proof_id: Uuid,
            proof_digest: Value,
        }
        let common = common_body(stored, DepositOperation::RecordPositionLossProof)?;
        let body: Body = serde_json::from_value(stored.body.clone()).map_err(corrupt_from)?;
        require_status(
            stored,
            &common,
            Self::new(
                CommandId::from_uuid(common.command_id),
                body.position_id,
                body.proof_id,
                parse_digest(body.proof_digest)?,
            ),
        )
    }
}

impl DepositReceipt for PositionLossReceipt {
    fn operation(&self) -> DepositOperation {
        DepositOperation::ConfirmPositionLoss
    }

    fn status(&self) -> i16 {
        200
    }

    fn disposition(&self) -> ReceiptDisposition {
        ReceiptDisposition::PositionLossRecorded
    }

    fn command_id(&self) -> CommandId {
        self.command_id
    }

    fn body(&self) -> Value {
        let mut body = common(self.operation(), self.disposition(), self.command_id);
        body.insert("listing_id".into(), json!(self.listing_id.to_string()));
        body.insert("listing_version".into(), json!(self.listing_version.value()));
        body.insert("state".into(), json!(self.state.as_str()));
        body.insert("position_id".into(), json!(self.position_id.to_string()));
        body.insert("position_version".into(), json!(self.position_version));
        body.insert("loss_fact_id".into(), json!(self.loss_fact_id.to_string()));
        body.insert("loss_case_id".into(), json!(self.loss_case_id.to_string()));
        body.insert("seller_receipt_id".into(), json!(self.seller_receipt_id.to_string()));
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
            position_id: Uuid,
            position_version: u64,
            loss_fact_id: Uuid,
            loss_case_id: Uuid,
            seller_receipt_id: Uuid,
        }
        let common = common_body(stored, DepositOperation::ConfirmPositionLoss)?;
        let body: Body = serde_json::from_value(stored.body.clone()).map_err(corrupt_from)?;
        if body.state != ListingState::Closed.as_str() || body.position_version == 0 {
            return Err(corrupt());
        }
        require_status(
            stored,
            &common,
            Self::new(
                CommandId::from_uuid(common.command_id),
                body.listing_id,
                ListingVersion::new(body.listing_version).map_err(corrupt_from)?,
                body.position_id,
                body.position_version,
                body.loss_fact_id,
                body.loss_case_id,
                body.seller_receipt_id,
            ),
        )
    }
}

impl DepositReceipt for LossCaseOutcomeReceipt {
    fn operation(&self) -> DepositOperation {
        DepositOperation::RecordLossCaseOutcome
    }

    fn status(&self) -> i16 {
        200
    }

    fn disposition(&self) -> ReceiptDisposition {
        ReceiptDisposition::CaseOutcomeRecorded
    }

    fn command_id(&self) -> CommandId {
        self.command_id
    }

    fn body(&self) -> Value {
        let mut body = common(self.operation(), self.disposition(), self.command_id);
        body.insert("case_id".into(), json!(self.case_id.to_string()));
        body.insert("case_version".into(), json!(self.case_version));
        body.insert("outcome_id".into(), json!(self.outcome_id.to_string()));
        body.insert("seller_receipt_id".into(), json!(self.seller_receipt_id.to_string()));
        finish_body(body)
    }

    fn decode(stored: &StoredReceipt) -> Result<Self, WriteError> {
        #[derive(Deserialize)]
        struct Body {
            case_id: Uuid,
            case_version: u64,
            outcome_id: Uuid,
            seller_receipt_id: Uuid,
        }
        let common = common_body(stored, DepositOperation::RecordLossCaseOutcome)?;
        let body: Body = serde_json::from_value(stored.body.clone()).map_err(corrupt_from)?;
        if body.case_version == 0 {
            return Err(corrupt());
        }
        require_status(
            stored,
            &common,
            Self::new(
                CommandId::from_uuid(common.command_id),
                body.case_id,
                body.case_version,
                body.outcome_id,
                body.seller_receipt_id,
            ),
        )
    }
}
