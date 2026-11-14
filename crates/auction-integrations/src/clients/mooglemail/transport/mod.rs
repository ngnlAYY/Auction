//! MoogleMail delivery transport 的职责分组。

use super::{
    MoogleMailCredentialSet, MoogleMailError, MoogleMailErrorCategory, MoogleMailOperation,
    MoogleMailSender, MoogleMailWorld, gateway::MoogleMailCredentialSession,
};

mod delivery;
mod maintenance;
mod outcomes;
mod payload;
mod types;

pub(crate) use outcomes::*;
pub(crate) use payload::*;
pub(crate) use types::*;
pub use types::{MoogleMailDeliveryTransport, MoogleMailNotificationTemplates};
