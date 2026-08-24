//! MoogleMail delivery transport 的职责分组。

use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc};

use auction_application::{
    ApplicationError, DeliverySendResult, DeliveryTransport, DeliveryTransportError,
    DeliveryWorkClaim, MailLoginKey, MailLoginKeyReader, MailProcessingError,
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use super::{
    MoogleMailActionState, MoogleMailClient, MoogleMailCredentialSet, MoogleMailError,
    MoogleMailErrorCategory, MoogleMailForm, MoogleMailMessageFields, MoogleMailOperation,
    MoogleMailParser, MoogleMailSendOutcome, MoogleMailSendReceipt, MoogleMailSendRequest,
    MoogleMailSender, MoogleMailWorld,
    protocol::{valid_decimal_id, valid_line, valid_opaque_id},
};

mod delivery;
mod outcomes;
mod payload;
mod types;

pub(crate) use outcomes::*;
pub(crate) use payload::*;
pub(crate) use types::*;
pub use types::{MoogleMailDeliveryTransport, MoogleMailNotificationTemplates};
