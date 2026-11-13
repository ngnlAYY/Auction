//! 用例、窄端口和事务边界。
//!
//! 公开 API 按限界上下文导出：公开面由 `ports`、`services` 中各上下文 `mod.rs` 的
//! `pub use` 清单决定，这里只做合并，不再维护第二份名单。

mod errors;
mod ports;
mod services;

// 只有端口或只有用例的上下文直接导出原模块；子模块按上下文归组后仍以原名字对外。
pub use ports::lot_watch::{pm as lot_watch_pm, reminders as lot_watch_reminders};
pub use ports::observability::{
    external_metrics, log_filter, notification_metrics, otlp_headers, sent_baseline, shadow_run,
    sli,
};
pub use ports::{delivery, external_accounts, forum, operations, scheduling};
pub use services::sessions;

pub mod core {
    pub use crate::errors::{
        ApplicationError, MailProcessingRetryability, SessionError, diagnostics,
    };
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
    mail_templates,
    player_notifications,
    public,
    realtime,
    reconciliation,
    recovery,
    runtime_switches,
    settlement,
);
