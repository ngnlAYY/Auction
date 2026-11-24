//! MoogleMail delivery transport 的职责分组。

mod delivery;
mod maintenance;
mod outcomes;
mod payload;
mod types;

pub(crate) use delivery::manifest_matches;
pub use delivery::{
    CONSIGNMENT_DISPOSITION_VALIDATOR_VERSION, DispositionAction, DispositionOutcome,
    DispositionProof, DispositionRequest, DispositionTarget, ExpectedReturn, NotSentReason,
    SentUnprovenReason, SignedProof, execute_prepared_consignment_disposition,
};
pub(crate) use outcomes::*;
pub use payload::decode_consignment_disposition_request;
pub(crate) use payload::*;
pub(crate) use types::*;
pub use types::{
    ExpectedAttachmentPayload, ExpectedMailManifestPayload, MailDeliveryPayload, MailKeyPayload,
    MoogleMailDeliveryTransport, MoogleMailNotificationTemplates,
};
