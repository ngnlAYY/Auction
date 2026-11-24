//! 测试回执包装对 v2 回执 trait 的委托。

use auction_application::consignment::{
    DepositOperation, EnvelopeReceipt, ListingStateChange, ObservationReceipt,
    OperationsHoldReceipt, ProofReceipt, ReceiptDisposition, RecheckReceipt, ReleaseReceipt,
    ReservationReceipt, ReservationRetired, SeasonCloseReceipt, SeasonNoticeReceipt, StartReceipt,
    WithdrawDepositReceipt,
};
use auction_domain::CommandId;
use serde_json::Value;

use super::TestDepositReceipt;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::ledger::StoredReceipt;
use crate::database::consignment::commands::receipts_v2::DepositReceipt;

pub(super) enum AnyReceipt<'a> {
    Reservation(&'a ReservationReceipt),
    ReservationRetired(&'a ReservationRetired),
    Envelope(&'a EnvelopeReceipt),
    Observation(&'a ObservationReceipt),
    Recheck(&'a RecheckReceipt),
    Release(&'a ReleaseReceipt),
    Start(&'a StartReceipt),
    Proof(&'a ProofReceipt),
    Withdraw(&'a WithdrawDepositReceipt),
    OperationsHold(&'a OperationsHoldReceipt),
    SeasonNotice(&'a SeasonNoticeReceipt),
    SeasonClose(&'a SeasonCloseReceipt),
}

pub(super) fn any(receipt: &TestDepositReceipt) -> AnyReceipt<'_> {
    match receipt {
        TestDepositReceipt::Reservation(receipt) => AnyReceipt::Reservation(receipt),
        TestDepositReceipt::ReservationRetired(receipt) => AnyReceipt::ReservationRetired(receipt),
        TestDepositReceipt::Envelope(receipt) => AnyReceipt::Envelope(receipt),
        TestDepositReceipt::Observation(receipt) => AnyReceipt::Observation(receipt),
        TestDepositReceipt::Recheck(receipt) => AnyReceipt::Recheck(receipt),
        TestDepositReceipt::Release(receipt) => AnyReceipt::Release(receipt),
        TestDepositReceipt::Start(receipt) => AnyReceipt::Start(receipt),
        TestDepositReceipt::Proof(receipt) => AnyReceipt::Proof(receipt),
        TestDepositReceipt::Withdraw(receipt) => AnyReceipt::Withdraw(receipt),
        TestDepositReceipt::OperationsHold(receipt) => AnyReceipt::OperationsHold(receipt),
        TestDepositReceipt::SeasonNotice(receipt) => AnyReceipt::SeasonNotice(receipt),
        TestDepositReceipt::SeasonClose(receipt) => AnyReceipt::SeasonClose(receipt),
    }
}

impl DepositReceipt for AnyReceipt<'_> {
    fn operation(&self) -> DepositOperation {
        match self {
            Self::Reservation(receipt) => receipt.operation(),
            Self::ReservationRetired(receipt) => receipt.operation(),
            Self::Envelope(receipt) => receipt.operation(),
            Self::Observation(receipt) => receipt.operation(),
            Self::Recheck(receipt) => receipt.operation(),
            Self::Release(receipt) => receipt.operation(),
            Self::Start(receipt) => receipt.operation(),
            Self::Proof(receipt) => receipt.operation(),
            Self::Withdraw(receipt) => receipt.operation(),
            Self::OperationsHold(receipt) => receipt.operation(),
            Self::SeasonNotice(receipt) => receipt.operation(),
            Self::SeasonClose(receipt) => receipt.operation(),
        }
    }

    fn status(&self) -> i16 {
        match self {
            Self::Reservation(receipt) => receipt.status(),
            Self::ReservationRetired(receipt) => receipt.status(),
            Self::Envelope(receipt) => receipt.status(),
            Self::Observation(receipt) => receipt.status(),
            Self::Recheck(receipt) => receipt.status(),
            Self::Release(receipt) => receipt.status(),
            Self::Start(receipt) => receipt.status(),
            Self::Proof(receipt) => receipt.status(),
            Self::Withdraw(receipt) => receipt.status(),
            Self::OperationsHold(receipt) => receipt.status(),
            Self::SeasonNotice(receipt) => receipt.status(),
            Self::SeasonClose(receipt) => receipt.status(),
        }
    }

    fn disposition(&self) -> ReceiptDisposition {
        match self {
            Self::Reservation(receipt) => receipt.disposition(),
            Self::ReservationRetired(receipt) => receipt.disposition(),
            Self::Envelope(receipt) => receipt.disposition(),
            Self::Observation(receipt) => receipt.disposition(),
            Self::Recheck(receipt) => receipt.disposition(),
            Self::Release(receipt) => receipt.disposition(),
            Self::Start(receipt) => receipt.disposition(),
            Self::Proof(receipt) => receipt.disposition(),
            Self::Withdraw(receipt) => receipt.disposition(),
            Self::OperationsHold(receipt) => receipt.disposition(),
            Self::SeasonNotice(receipt) => receipt.disposition(),
            Self::SeasonClose(receipt) => receipt.disposition(),
        }
    }

    fn command_id(&self) -> CommandId {
        match self {
            Self::Reservation(receipt) => receipt.command_id(),
            Self::ReservationRetired(receipt) => receipt.command_id(),
            Self::Envelope(receipt) => receipt.command_id(),
            Self::Observation(receipt) => receipt.command_id(),
            Self::Recheck(receipt) => receipt.command_id(),
            Self::Release(receipt) => receipt.command_id(),
            Self::Start(receipt) => receipt.command_id(),
            Self::Proof(receipt) => receipt.command_id(),
            Self::Withdraw(receipt) => receipt.command_id(),
            Self::OperationsHold(receipt) => receipt.command_id(),
            Self::SeasonNotice(receipt) => receipt.command_id(),
            Self::SeasonClose(receipt) => receipt.command_id(),
        }
    }

    fn body(&self) -> Value {
        match self {
            Self::Reservation(receipt) => receipt.body(),
            Self::ReservationRetired(receipt) => receipt.body(),
            Self::Envelope(receipt) => receipt.body(),
            Self::Observation(receipt) => receipt.body(),
            Self::Recheck(receipt) => receipt.body(),
            Self::Release(receipt) => receipt.body(),
            Self::Start(receipt) => receipt.body(),
            Self::Proof(receipt) => receipt.body(),
            Self::Withdraw(receipt) => receipt.body(),
            Self::OperationsHold(receipt) => receipt.body(),
            Self::SeasonNotice(receipt) => receipt.body(),
            Self::SeasonClose(receipt) => receipt.body(),
        }
    }

    fn listing_changes(&self) -> Vec<ListingStateChange> {
        match self {
            Self::Reservation(receipt) => receipt.listing_changes(),
            Self::ReservationRetired(receipt) => receipt.listing_changes(),
            Self::Envelope(receipt) => receipt.listing_changes(),
            Self::Observation(receipt) => receipt.listing_changes(),
            Self::Recheck(receipt) => receipt.listing_changes(),
            Self::Release(receipt) => receipt.listing_changes(),
            Self::Start(receipt) => receipt.listing_changes(),
            Self::Proof(receipt) => receipt.listing_changes(),
            Self::Withdraw(receipt) => receipt.listing_changes(),
            Self::OperationsHold(receipt) => receipt.listing_changes(),
            Self::SeasonNotice(receipt) => receipt.listing_changes(),
            Self::SeasonClose(receipt) => receipt.listing_changes(),
        }
    }

    fn decode(_stored: &StoredReceipt) -> Result<Self, WriteError> {
        Err(WriteError::invariant("consignment_deposit_test_receipt"))
    }
}
