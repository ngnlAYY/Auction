//! MoogleMail delivery transport 的职责分组。

mod delivery;
mod maintenance;
mod outcomes;
mod payload;
mod types;

pub(crate) use outcomes::*;
pub(crate) use payload::*;
pub(crate) use types::*;
pub use types::{MoogleMailDeliveryTransport, MoogleMailNotificationTemplates};
