//! 整封决定、观察、隔离重检与解除回执的账本映射。

use auction_application::consignment::{
    DEPOSIT_SCHEMA_VERSION, DepositOperation, EnvelopeReceipt, ListingStateChange,
    ObservationReceipt, ReceiptDisposition, RecheckReceipt, ReleaseReceipt, ReturnIntakeReference,
};
use auction_domain::CommandId;
use auction_domain::consignment::deposit::ReleaseKind;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::DepositReceipt;
use super::common;
use super::common_body;
use super::corrupt;
use super::corrupt_from;
use super::decode_listing_changes;
use super::digest;
use super::finish_body;
use super::listing_changes;
use super::parse_digest;
use super::require_status;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::ledger::StoredReceipt;

impl DepositReceipt for EnvelopeReceipt {
    fn operation(&self) -> DepositOperation {
        DepositOperation::DecideEnvelope
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
        body.insert("listing_changes".into(), listing_changes(&self.listing_changes));
        body.insert(
            "quarantine".into(),
            match self.quarantine {
                Some(quarantine) => json!({
                    "quarantine_id": quarantine.quarantine_id.to_string(),
                    "version": quarantine.version,
                }),
                None => Value::Null,
            },
        );
        // 接管分支与直接接管的 `return_acquired` 回执同形，字段平铺在正文里。
        if let Some(intake) = self.return_intake {
            body.insert("return_intake_id".into(), json!(intake.return_intake_id.to_string()));
            body.insert("original_round".into(), json!(intake.original_round));
            body.insert("take_work_id".into(), json!(intake.take_work_id.to_string()));
            body.insert("take_delivery_id".into(), json!(intake.take_delivery_id.to_string()));
        }
        finish_body(body)
    }

    fn listing_changes(&self) -> Vec<ListingStateChange> {
        self.listing_changes.clone()
    }

    fn decode(stored: &StoredReceipt) -> Result<Self, WriteError> {
        #[derive(Deserialize)]
        struct Body {
            decision_id: Uuid,
            listing_changes: Option<Value>,
            quarantine: Option<Value>,
            return_intake_id: Option<Uuid>,
            original_round: Option<u32>,
            take_work_id: Option<Uuid>,
            take_delivery_id: Option<Uuid>,
        }
        #[derive(Deserialize)]
        struct Quarantine {
            quarantine_id: Uuid,
            version: u64,
        }
        let common = common_body(stored, DepositOperation::DecideEnvelope)?;
        let body: Body = serde_json::from_value(stored.body.clone()).map_err(corrupt_from)?;
        let disposition = ReceiptDisposition::ALL
            .into_iter()
            .find(|disposition| disposition.as_str() == common.disposition)
            .ok_or_else(corrupt)?;
        let quarantine = match body.quarantine {
            None | Some(Value::Null) => None,
            Some(value) => {
                let quarantine: Quarantine = serde_json::from_value(value).map_err(corrupt_from)?;
                Some(auction_application::consignment::QuarantineReference {
                    quarantine_id: quarantine.quarantine_id,
                    version: quarantine.version,
                })
            }
        };
        let return_intake = match (
            body.return_intake_id,
            body.original_round,
            body.take_work_id,
            body.take_delivery_id,
        ) {
            (None, None, None, None) => None,
            (
                Some(return_intake_id),
                Some(original_round),
                Some(take_work_id),
                Some(take_delivery_id),
            ) => Some(ReturnIntakeReference {
                return_intake_id,
                original_round,
                take_work_id,
                take_delivery_id,
            }),
            _ => return Err(corrupt()),
        };
        require_status(
            stored,
            &common,
            Self {
                schema_version: DEPOSIT_SCHEMA_VERSION,
                operation: DepositOperation::DecideEnvelope,
                disposition,
                command_id: CommandId::from_uuid(common.command_id),
                decision_id: body.decision_id,
                listing_changes: decode_listing_changes(body.listing_changes)?,
                quarantine,
                return_intake,
            },
        )
    }
}

impl DepositReceipt for ObservationReceipt {
    fn operation(&self) -> DepositOperation {
        DepositOperation::AppendQuarantineObservation
    }

    fn status(&self) -> i16 {
        200
    }

    fn disposition(&self) -> ReceiptDisposition {
        ReceiptDisposition::ObservationAppended
    }

    fn command_id(&self) -> CommandId {
        self.command_id
    }

    fn body(&self) -> Value {
        let mut body = common(self.operation(), self.disposition(), self.command_id);
        body.insert("quarantine_id".into(), json!(self.quarantine_id.to_string()));
        body.insert("quarantine_version".into(), json!(self.quarantine_version));
        body.insert("observation_id".into(), json!(self.observation_id.to_string()));
        body.insert("digest".into(), digest(self.digest));
        finish_body(body)
    }

    fn decode(stored: &StoredReceipt) -> Result<Self, WriteError> {
        #[derive(Deserialize)]
        struct Body {
            quarantine_id: Uuid,
            quarantine_version: u64,
            observation_id: Uuid,
            digest: Value,
        }
        let common = common_body(stored, DepositOperation::AppendQuarantineObservation)?;
        let body: Body = serde_json::from_value(stored.body.clone()).map_err(corrupt_from)?;
        require_status(
            stored,
            &common,
            Self::appended(
                CommandId::from_uuid(common.command_id),
                body.quarantine_id,
                body.quarantine_version,
                body.observation_id,
                parse_digest(body.digest)?,
            ),
        )
    }
}

impl DepositReceipt for RecheckReceipt {
    fn operation(&self) -> DepositOperation {
        DepositOperation::RecheckQuarantine
    }

    fn status(&self) -> i16 {
        202
    }

    fn disposition(&self) -> ReceiptDisposition {
        ReceiptDisposition::RecheckQueued
    }

    fn command_id(&self) -> CommandId {
        self.command_id
    }

    fn body(&self) -> Value {
        let mut body = common(self.operation(), self.disposition(), self.command_id);
        body.insert("quarantine_id".into(), json!(self.quarantine_id.to_string()));
        body.insert("work_id".into(), json!(self.work_id.to_string()));
        finish_body(body)
    }

    fn decode(stored: &StoredReceipt) -> Result<Self, WriteError> {
        #[derive(Deserialize)]
        struct Body {
            quarantine_id: Uuid,
            work_id: Uuid,
        }
        let common = common_body(stored, DepositOperation::RecheckQuarantine)?;
        let body: Body = serde_json::from_value(stored.body.clone()).map_err(corrupt_from)?;
        require_status(
            stored,
            &common,
            Self::queued(CommandId::from_uuid(common.command_id), body.quarantine_id, body.work_id),
        )
    }
}

impl DepositReceipt for ReleaseReceipt {
    fn operation(&self) -> DepositOperation {
        DepositOperation::ReleaseQuarantine
    }

    fn status(&self) -> i16 {
        200
    }

    fn disposition(&self) -> ReceiptDisposition {
        ReceiptDisposition::QuarantineReleased
    }

    fn command_id(&self) -> CommandId {
        self.command_id
    }

    fn body(&self) -> Value {
        let mut body = common(self.operation(), self.disposition(), self.command_id);
        body.insert("quarantine_id".into(), json!(self.quarantine_id.to_string()));
        body.insert("release_id".into(), json!(self.release_id.to_string()));
        body.insert("decision_id".into(), json!(self.decision_id.to_string()));
        body.insert("decision_kind".into(), json!(self.decision_kind.as_str()));
        if self.decision_kind == ReleaseKind::Accept {
            body.insert("listing_changes".into(), listing_changes(&self.listing_changes));
        }
        if self.decision_kind == ReleaseKind::AuctionReturnCase {
            body.insert(
                "obligation_id".into(),
                self.obligation_id.map_or(Value::Null, |value| json!(value.to_string())),
            );
            if let Some(value) = self.return_mail_id {
                body.insert("return_mail_id".into(), json!(value.to_string()));
            }
            if let Some(value) = self.return_evidence_id {
                body.insert("return_evidence_id".into(), json!(value.to_string()));
            }
        }
        if self.decision_kind == ReleaseKind::AuctionRecoveryReturn {
            for (name, value) in [
                ("obligation_id", self.obligation_id),
                ("return_mail_id", self.return_mail_id),
                ("return_evidence_id", self.return_evidence_id),
                ("recovery_id", self.recovery_id),
                ("recovery_handoff_id", self.recovery_handoff_id),
            ] {
                if let Some(value) = value {
                    body.insert(name.into(), json!(value.to_string()));
                }
            }
        }
        if self.decision_kind == ReleaseKind::AcquireReturn {
            if let Some(value) = self.return_intake_id {
                body.insert("return_intake_id".into(), json!(value.to_string()));
            }
            if let Some(value) = self.original_round {
                body.insert("original_round".into(), json!(value));
            }
            if let Some(value) = self.take_work_id {
                body.insert("take_work_id".into(), json!(value.to_string()));
            }
            if let Some(value) = self.take_delivery_id {
                body.insert("take_delivery_id".into(), json!(value.to_string()));
            }
        }
        finish_body(body)
    }

    fn listing_changes(&self) -> Vec<ListingStateChange> {
        self.listing_changes.clone()
    }

    fn decode(stored: &StoredReceipt) -> Result<Self, WriteError> {
        #[derive(Deserialize)]
        struct Body {
            quarantine_id: Uuid,
            release_id: Uuid,
            decision_id: Uuid,
            decision_kind: String,
            listing_changes: Option<Value>,
            obligation_id: Option<Uuid>,
            return_mail_id: Option<Uuid>,
            return_evidence_id: Option<Uuid>,
            recovery_id: Option<Uuid>,
            recovery_handoff_id: Option<Uuid>,
            return_intake_id: Option<Uuid>,
            original_round: Option<u32>,
            take_work_id: Option<Uuid>,
            take_delivery_id: Option<Uuid>,
        }
        let common = common_body(stored, DepositOperation::ReleaseQuarantine)?;
        let body: Body = serde_json::from_value(stored.body.clone()).map_err(corrupt_from)?;
        let decision_kind = ReleaseKind::parse(&body.decision_kind).ok_or_else(corrupt)?;
        require_status(
            stored,
            &common,
            Self {
                schema_version: DEPOSIT_SCHEMA_VERSION,
                operation: DepositOperation::ReleaseQuarantine,
                disposition: ReceiptDisposition::QuarantineReleased,
                command_id: CommandId::from_uuid(common.command_id),
                quarantine_id: body.quarantine_id,
                release_id: body.release_id,
                decision_id: body.decision_id,
                decision_kind,
                listing_changes: decode_listing_changes(body.listing_changes)?,
                obligation_id: body.obligation_id,
                return_mail_id: body.return_mail_id,
                return_evidence_id: body.return_evidence_id,
                recovery_id: body.recovery_id,
                recovery_handoff_id: body.recovery_handoff_id,
                return_intake_id: body.return_intake_id,
                original_round: body.original_round,
                take_work_id: body.take_work_id,
                take_delivery_id: body.take_delivery_id,
            },
        )
    }
}
