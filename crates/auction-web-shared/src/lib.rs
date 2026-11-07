//! Public 与 Admin 共用的浏览器端基础能力。
//! 只放真正通用、无业务偏向的内容：设计变量与基础排版、语言与主题选择、无领域语义的展示组件、
//! 通用交互原语（弹窗外壳、两阶段确认命令状态机）、页面可见性感知等待、同源请求传输边界、
//! URL query 同步、wire 格式谓词与稳定错误码分类。
//! 面向公开竞价或管理命令的组件必须留在各自 surface crate，避免 Admin 专属代码进入
//! 公开端 WASM 产物。本 crate 不得依赖 `auction-storage`、`auction-config` 或任何凭据访问路径。

mod a11y;
mod bootstrap;
mod build_notice;
mod clock;
mod commands;
mod components;
mod errors;
mod format;
pub mod i18n;
mod media;
mod query;
mod theme;
mod transport;
mod validation;
mod visibility;

pub use i18n::{LanguagePicker, SUPPORTED_LOCALES, SupportedLocale, use_locale_effects};
pub use theme::{THEME_STORAGE_KEY, Theme, ThemePicker, use_theme_effects};

pub use a11y::{use_dialog_focus_trap, use_route_focus};
pub use bootstrap::{
    BROWSER_ERROR_ROUTE, BrowserErrorClass, build_id, build_id_is_valid, install_panic_hook,
    report_browser_error, report_browser_error_with_build_id, use_app_boot_ready,
};
pub use build_notice::{
    BuildDriftWait, SERVER_BUILD_HEADER, StaleBuildBanner, build_is_stale, observe_server_build_id,
    use_build_drift, wait_for_build_drift,
};
pub use clock::{SecondTicker, use_second_tick, use_second_ticker};
pub use commands::{ConfirmCommand, use_confirm_command};
pub use errors::{BaseErrorKind, base_api_error_message, classify_base_error};
pub use query::{
    QueryState, QueryWriteMode, read_query_pairs, use_popstate_revision, use_query_state,
    write_query_pairs,
};
pub use transport::{
    BrowserLock, BrowserLockError, DEFAULT_REQUEST_TIMEOUT_MILLIS, TransportError, send_get,
    send_get_observed, send_request, send_request_observed, send_request_with_timeout,
};

pub use components::{
    AsyncState, AsyncStateKind, BASE_CSS, BaseStylesheet, Dialog, MarkdownBlocks, MarkdownDocument,
    Pager, TableColumns, Time, TruncatedText, without_leading_title,
};
pub use format::{
    format_credit_amount, format_credit_amount_full, format_held_duration, format_ppm,
};
pub use validation::is_legacy_uuid;
pub use visibility::{PageVisibility, sleep_while_visible};
