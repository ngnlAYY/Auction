//! MoogleMail delivery 的 payload、模板和 transport 状态模型。

mod delivery;
mod settlement;
mod templates;
mod transport;

/// 旧来源（通知与结算）的冻结载荷版本。
pub(crate) const MAIL_DELIVERY_SCHEMA_VERSION: u8 = 1;
/// 寄售处置载荷版本；与旧来源分开命名，避免任一版本变化误放宽另一来源。
pub(crate) const CONSIGNMENT_DISPOSITION_SCHEMA_VERSION: u8 = 2;

pub(crate) use delivery::ActionExpectation;
pub use delivery::{
    ExpectedAttachmentPayload, ExpectedMailManifestPayload, MailDeliveryPayload, MailKeyPayload,
};
pub(crate) use settlement::{
    SettlementDeliveryPayload, SettlementEquipmentSecretReference, SettlementReturnWirePayload,
    SettlementSendWirePayload,
};
pub use templates::MoogleMailNotificationTemplates;
pub use transport::MoogleMailDeliveryTransport;
