//! 用例、窄端口和事务边界。
//!
//! 公开 API 按限界上下文导出：公开面由 `ports`、`services` 中各上下文 `mod.rs` 的
//! `pub use` 清单决定，这里只做合并，不再维护第二份名单。

mod errors;
mod operator_audit;
mod ports;
mod services;

// 只有端口或只有用例的上下文直接导出原模块。
pub use ports::{
    delivery, external_accounts, external_metrics, forum, lot_watch_pm, lot_watch_reminders,
    notification_metrics, operations, scheduling, sent_baseline, shadow_run, sli,
};
pub use services::sessions;

pub mod core {
    pub use crate::errors::{
        ApplicationError, MailProcessingRetryability, SessionError, diagnostics,
    };
    pub use crate::operator_audit::{CliOperatorIdentity, cli_audit_metadata};
    pub use crate::ports::core::*;
}

/// 合并同一上下文的端口与用例公开项。
macro_rules! merged_contexts {
    ($($context:ident),+ $(,)?) => {$(
        pub mod $context {
            pub use crate::ports::$context::*;
            pub use crate::services::$context::*;
        }
    )+};
}

merged_contexts!(
    admin,
    auction,
    mail,
    player_notifications,
    public,
    realtime,
    reconciliation,
    recovery,
    runtime_switches,
    settlement,
);
