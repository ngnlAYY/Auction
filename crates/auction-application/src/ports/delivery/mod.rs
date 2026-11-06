//! delivery 上下文。

pub(crate) mod admin;
pub(crate) mod admin_guard;
pub(crate) mod cadence;
pub(crate) mod detail;
pub(crate) mod disposition;
pub(crate) mod expectation;
pub(crate) mod sent_admission;
pub(crate) mod sent_confirmation;
pub(crate) mod support;
pub(crate) mod worker;

pub use admin::{
    DELIVERY_ADMIN_CATEGORIES, DELIVERY_ADMIN_RETIREMENT_REASONS,
    DELIVERY_ADMIN_SUPPRESSION_REASONS, DELIVERY_ADMIN_WORLDS, DeliveryAdminAction,
    DeliveryAdminCommand, DeliveryAdminError, DeliveryAdminListRequest, DeliveryAdminPage,
    DeliveryAdminPort, DeliveryAdminResult, DeliveryAdminSnapshot, MAX_DELIVERY_ADMIN_OFFSET,
    MAX_DELIVERY_ADMIN_PAGE_SIZE, MAX_DELIVERY_ADMIN_RECIPIENT_LEN,
    MAX_DELIVERY_ADMIN_TEMPLATE_LEN, MAX_DELIVERY_COD,
};
pub use admin_guard::{
    DeliveryAdminGuardBlocker, DeliveryAdminGuardDecision, DeliveryAdminGuardSnapshot,
    DeliveryAdminTransition,
};
pub use cadence::{DeliverySendIntervals, DeliveryWakeup};
pub use detail::{
    DeliveryAdminAttempt, DeliveryAdminCustody, DeliveryAdminDetail, DeliveryAdminDisposition,
    DeliveryAdminObligation, DeliveryAdminRecovery, DeliveryAdminSentEvidence,
};
pub use disposition::*;
pub use expectation::{
    DeliveryPreparedExpectation, DeliverySendCredentialBinding, DeliverySendExpectation,
    DeliveryTakeExpectation,
};
pub use sent_admission::*;
pub use sent_confirmation::*;
pub use support::{
    MAX_PLAYER_SUPPORT_ROWS, MAX_PLAYER_SUPPORT_USER_LEN, PlayerSupportBid, PlayerSupportBundle,
    PlayerSupportDeal, PlayerSupportNotification, PlayerSupportObligation, PlayerSupportReference,
    PlayerSupportRequest, PlayerSupportSelector, PlayerSupportUser,
};
pub use worker::{
    AuthorizedDeliveryPreparation, AuthorizedDeliverySend, DeliveryClaimExclusions,
    DeliveryFenceOutcome, DeliveryPreparation, DeliveryPreparedSend, DeliveryRecipientExclusion,
    DeliverySendResult, DeliveryTransport, DeliveryTransportError, DeliveryWorkClaim,
    DeliveryWorkStore,
};
