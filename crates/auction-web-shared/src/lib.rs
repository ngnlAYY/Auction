//! Public 与 Admin 共用的浏览器端基础能力。
//! 只放真正通用、无业务偏向的内容：设计变量与基础排版、语言选择、无领域语义的展示组件、
//! 通用交互原语（弹窗外壳、两阶段确认命令状态机）、同源请求传输边界、URL query 同步与
//! 稳定错误码分类。
//! 面向公开竞价或管理命令的组件必须留在各自 surface crate，避免 Admin 专属代码进入
//! 公开端 WASM 产物。本 crate 不得依赖 `auction-storage`、`auction-config` 或任何凭据访问路径。

mod a11y;
mod bootstrap;
mod clock;
mod commands;
mod components;
mod errors;
mod format;
pub mod i18n;
mod media;
mod query;
mod transport;

pub use i18n::{
    LanguagePicker, SUPPORTED_LOCALES, SupportedLocale, format_localized_time, use_locale_effects,
};

pub use a11y::{use_dialog_focus_trap, use_route_focus};
pub use bootstrap::{install_panic_hook, use_app_boot_ready};
pub use clock::{SecondTicker, use_second_tick, use_second_ticker};
pub use commands::{ConfirmCommand, use_confirm_command};
pub use errors::{BaseErrorKind, base_api_error_message, classify_base_error};
pub use media::use_media_query;
pub use query::{QueryState, QueryWriteMode, read_query_pairs, use_query_state, write_query_pairs};
pub use transport::{
    BrowserLock, BrowserLockError, DEFAULT_REQUEST_TIMEOUT_MILLIS, TransportError, send_get,
    send_get_observed, send_request, send_request_observed, send_request_with_timeout,
};

pub use components::{
    AsyncState, AsyncStateKind, BASE_CSS, BaseStylesheet, Dialog, MarkdownBlocks, MarkdownDocument,
    Pager, TableColumns, Time, TruncatedText, without_leading_title,
};
pub use format::{format_credit_amount, format_credit_amount_full, format_held_duration};
