//! MoogleMail delivery transport 的职责分组。

mod delivery;
mod maintenance;
mod outcomes;
mod payload;
mod types;

pub(crate) use delivery::manifest_matches;
pub use delivery::{
    CONSIGNMENT_DISPATCH_COLLECTED_VALIDATOR_VERSION, CONSIGNMENT_DISPATCH_SENT_VALIDATOR_VERSION,
    CONSIGNMENT_DISPOSITION_VALIDATOR_VERSION, ConsignmentCollection, ConsignmentCollectionTarget,
    ConsignmentDispatchMail, ConsignmentDispatchOutcome, ConsignmentDispatchRecheck,
    ConsignmentDispatchRecheckTarget, ConsignmentDispatchSent, DispositionAction,
    DispositionOutcome, DispositionProof, DispositionRequest, DispositionTarget, ExpectedReturn,
    NotSentReason, PlainNoticeMail, PlainNoticeOutcome, PreparedConsignmentDispatch,
    SentUnprovenReason, SignedProof, execute_prepared_consignment_disposition,
};
pub(crate) use outcomes::*;
pub(crate) use payload::*;
pub use payload::{
    DownstreamDispatchFacts, FirstDispatchFacts, RenderedDispatchMail, WithdrawalDispatchFacts,
    decode_consignment_disposition_request, render_consignment_send, render_downstream_dispatch,
    render_first_dispatch, render_frozen_dispatch, render_withdrawal_dispatch,
};
pub(crate) use types::*;
pub use types::{
    ExpectedAttachmentPayload, ExpectedMailManifestPayload, MailDeliveryPayload, MailKeyPayload,
    MoogleMailDeliveryTransport, MoogleMailNotificationTemplates,
};
