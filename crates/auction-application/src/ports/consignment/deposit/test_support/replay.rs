//! 内存假实现共用的只读命令重放与回执解码。

use auction_domain::CommandId;

use super::{FakeDepositStore, LedgerReceipt};
use crate::ports::consignment::ConsignmentError;
use crate::ports::consignment::deposit::{
    DepositOperation, EnvelopeReceipt, ObservationReceipt, OperationsHoldReceipt, ProofReceipt,
    RecheckReceipt, ReleaseReceipt, ReservationReceipt, ReservationRetired, SeasonCloseReceipt,
    SeasonNoticeReceipt, WithdrawDepositReceipt,
};
use crate::ports::core::RequestHash;

impl FakeDepositStore {
    pub(super) fn replay<Receipt>(
        &self,
        command_id: CommandId,
        operation: DepositOperation,
        request_hash: RequestHash,
        decode: impl FnOnce(LedgerReceipt) -> Result<Receipt, ConsignmentError>,
    ) -> Result<Option<Receipt>, ConsignmentError> {
        let state = self.lock();
        match state.ledger.get(&command_id) {
            None => Ok(None),
            Some(record)
                if record.operation == operation && record.request_hash == request_hash =>
            {
                decode(record.receipt.clone()).map(Some)
            }
            Some(_) => Err(ConsignmentError::CommandIdConflict),
        }
    }

    pub(super) fn replay_reservation(
        receipt: LedgerReceipt,
    ) -> Result<ReservationReceipt, ConsignmentError> {
        match receipt {
            LedgerReceipt::Reservation(receipt) => Ok(receipt),
            _ => Err(ConsignmentError::Unavailable),
        }
    }

    pub(super) fn replay_retired(
        receipt: LedgerReceipt,
    ) -> Result<ReservationRetired, ConsignmentError> {
        match receipt {
            LedgerReceipt::Retired(receipt) => Ok(receipt),
            _ => Err(ConsignmentError::Unavailable),
        }
    }

    pub(super) fn replay_envelope(
        receipt: LedgerReceipt,
    ) -> Result<EnvelopeReceipt, ConsignmentError> {
        match receipt {
            LedgerReceipt::Envelope(receipt) => Ok(receipt),
            _ => Err(ConsignmentError::Unavailable),
        }
    }

    pub(super) fn replay_observation(
        receipt: LedgerReceipt,
    ) -> Result<ObservationReceipt, ConsignmentError> {
        match receipt {
            LedgerReceipt::Observation(receipt) => Ok(receipt),
            _ => Err(ConsignmentError::Unavailable),
        }
    }

    pub(super) fn replay_recheck(
        receipt: LedgerReceipt,
    ) -> Result<RecheckReceipt, ConsignmentError> {
        match receipt {
            LedgerReceipt::Recheck(receipt) => Ok(receipt),
            _ => Err(ConsignmentError::Unavailable),
        }
    }

    pub(super) fn replay_release(
        receipt: LedgerReceipt,
    ) -> Result<ReleaseReceipt, ConsignmentError> {
        match receipt {
            LedgerReceipt::Release(receipt) => Ok(receipt),
            _ => Err(ConsignmentError::Unavailable),
        }
    }

    pub(super) fn replay_proof(receipt: LedgerReceipt) -> Result<ProofReceipt, ConsignmentError> {
        match receipt {
            LedgerReceipt::Proof(receipt) => Ok(receipt),
            _ => Err(ConsignmentError::Unavailable),
        }
    }

    pub(super) fn replay_withdraw(
        receipt: LedgerReceipt,
    ) -> Result<WithdrawDepositReceipt, ConsignmentError> {
        match receipt {
            LedgerReceipt::Withdraw(receipt) => Ok(receipt),
            _ => Err(ConsignmentError::Unavailable),
        }
    }

    pub(super) fn replay_hold(
        receipt: LedgerReceipt,
    ) -> Result<OperationsHoldReceipt, ConsignmentError> {
        match receipt {
            LedgerReceipt::Hold(receipt) => Ok(receipt),
            _ => Err(ConsignmentError::Unavailable),
        }
    }

    pub(super) fn replay_notice(
        receipt: LedgerReceipt,
    ) -> Result<SeasonNoticeReceipt, ConsignmentError> {
        match receipt {
            LedgerReceipt::Notice(receipt) => Ok(receipt),
            _ => Err(ConsignmentError::Unavailable),
        }
    }

    pub(super) fn replay_close(
        receipt: LedgerReceipt,
    ) -> Result<SeasonCloseReceipt, ConsignmentError> {
        match receipt {
            LedgerReceipt::Close(receipt) => Ok(receipt),
            _ => Err(ConsignmentError::Unavailable),
        }
    }
}
