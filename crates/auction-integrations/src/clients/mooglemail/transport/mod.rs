//! MoogleMail delivery transport 的职责分组。

use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc};

use auction_application::{
    ApplicationError, DeliveryPreparation, DeliveryPreparedSend, DeliverySendResult,
    DeliveryTransport, DeliveryTransportError, DeliveryWorkClaim, MailLoginKey, MailLoginKeyReader,
    MailProcessingError, SettlementFulfillmentEquipmentSecretReader,
};
use auction_domain::EquipmentId;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::EquipmentKeyOpener;

use super::{
    MoogleMailActionState, MoogleMailClient, MoogleMailCredentialSet, MoogleMailError,
    MoogleMailErrorCategory, MoogleMailForm, MoogleMailMessageFields, MoogleMailOperation,
    MoogleMailParser, MoogleMailSendOutcome, MoogleMailSendReceipt, MoogleMailSendRequest,
    MoogleMailSender, MoogleMailWorld,
    forms::valid_body,
    gateway::MoogleMailCredentialSession,
    protocol::{valid_decimal_id, valid_line, valid_opaque_id},
    sender::MoogleMailSentBaseline,
};

mod delivery;
mod maintenance;
mod outcomes;
#[path = "payload/mod.rs"]
mod payload;
#[path = "types/mod.rs"]
mod types;

pub(crate) use outcomes::*;
pub(crate) use payload::*;
pub(crate) use types::*;
pub use types::{MoogleMailDeliveryTransport, MoogleMailNotificationTemplates};
