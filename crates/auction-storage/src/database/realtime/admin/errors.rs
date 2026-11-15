//! Admin realtime 的业务错误映射；存储错误直接由 `crate::database::storage_error` 构造。

use auction_application::core::ApplicationError;
use auction_application::realtime::RealtimeAdminError;

pub(super) fn admin_error(error: RealtimeAdminError) -> ApplicationError {
    ApplicationError::RealtimeAdmin(error)
}
