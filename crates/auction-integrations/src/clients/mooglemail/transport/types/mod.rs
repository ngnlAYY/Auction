//! MoogleMail delivery 的 payload、模板和 transport 状态模型。

mod delivery;
mod settlement;
mod templates;
mod transport;

pub(crate) const SCHEMA_VERSION: u8 = 1;
pub(crate) const MAX_ATTACHMENT_IDS: usize = 1000;

pub(crate) use delivery::{ActionExpectation, MailDeliveryPayload};
pub(crate) use settlement::{
    SettlementDeliveryPayload, SettlementEquipmentSecretReference, SettlementReturnWirePayload,
    SettlementSendWirePayload,
};
pub use templates::MoogleMailNotificationTemplates;
pub use transport::MoogleMailDeliveryTransport;
