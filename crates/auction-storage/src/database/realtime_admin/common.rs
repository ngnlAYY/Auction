//! Admin realtime 的统一存储错误映射。

pub(super) use crate::database::storage_error;
use auction_application::{ApplicationError, RealtimeAdminError};

pub(super) fn admin_error(error: RealtimeAdminError) -> ApplicationError {
    ApplicationError::RealtimeAdmin(error)
}
