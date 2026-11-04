//! MoogleMail gateway 的稳定出口；单 Cookie 会话、扫描模型和读取由子模块持有，Cookie
//! 不提升到应用层。

use super::*;

const MAX_HARD_PAGES: usize = 100;
const MAX_FETCH_LIMIT: usize = 1000;

mod credentials;
mod internal;
mod model;
mod operation_lock;
mod reader;
mod sent_evidence;

pub(crate) use credentials::MoogleMailCredentialSession;
pub use credentials::{MoogleMailCredentialSet, MoogleMailRuntimeCredential};
pub use model::{
    MoogleMailGatewayConfig, MoogleMailPage, MoogleMailSentLookupReport,
    MoogleMailSentLookupRequest, MoogleMailSentLookupStatus,
};
pub(crate) use operation_lock::{OperationLock, OperationLockGuard};
pub use reader::MoogleMailGateway;

fn configuration(world: MoogleMailWorld) -> MoogleMailError {
    operation_error(MoogleMailOperation::Inbox, world, MoogleMailErrorCategory::Configuration)
}

fn protocol_error(world: MoogleMailWorld) -> MoogleMailError {
    operation_error(MoogleMailOperation::Inbox, world, MoogleMailErrorCategory::ProtocolDrift)
}

fn operation_error(
    operation: MoogleMailOperation,
    world: MoogleMailWorld,
    category: MoogleMailErrorCategory,
) -> MoogleMailError {
    MoogleMailError::new(operation, world, category, false)
}
