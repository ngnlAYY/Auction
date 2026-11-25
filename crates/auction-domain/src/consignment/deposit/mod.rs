//! 寄售第 4 期收件与代管：预约令牌、整封决定、隔离解除、取得证明、挂牌决策。

mod actions;
mod custody;
mod envelope;
mod errors;
mod listing_decision;
mod release;
mod token;

pub use actions::{ActionFacts, AdminAction, SellerAction, admin_actions, seller_actions};
pub use custody::{
    CustodyTransition, ExpectedAttachment, PositionFacts, PositionState, ProofValidatorStatus,
    RemovedMarker, ValidatedMailProof, apply_custody_proof_batch,
};
pub use envelope::{
    EnvelopeCapabilities, EnvelopeDecision, EnvelopeItemKind, EnvelopeItemObservation,
    EnvelopeObservation, EnvelopeRoute, LegacyAuctionOwner, LegacyReturnEvidence,
    LockedEnvelopeFacts, MatchedDepositItem, PendingReason, QuarantineReason, ReservationBinding,
    ReservationState, ReturnItem, ReturnItemReason, ReturnManifest, SenderIdentity,
    TrustedConsignmentRts, decide_envelope, decide_envelope_with_origin, require_asset_count,
    reservation_is_live, reservation_matches_item, reservation_sort_key, same_mail_key,
};
pub use errors::DepositError;
pub use listing_decision::{
    ListingDecision, ListingDecisionFacts, WithdrawalIntentOutcome, decide_listing_after_custody,
    decide_withdrawal_intent, operations_hold_reason, release_operations_hold_reason,
};
pub use release::{
    ACQUIRE_RETURN_PHASE4_REJECTION, AuctionRecoveryEvidence, AuctionRecoveryKind,
    AuctionReturnEvidence, AuctionReturnKind, LockedReleaseFacts, ReleaseKind, ReleasePlan,
    ReleaseSource, classify_release,
};
pub use token::{
    DEPOSIT_BYTE_LIMIT, DEPOSIT_ITEM_LIMIT, ReservationToken, TOKEN_BODY_LEN, TOKEN_PREFIX,
    TOKEN_RANDOM_BYTES, TOKEN_TEXT_LEN, TokenDigest, TokenText, format_token, is_consign_subject,
    parse_deposit_body, parse_token, token_digest,
};
