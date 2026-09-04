//! Public 与 Admin 共用的浏览器端基础能力。
//!
//! 只允许放真正通用、没有业务偏向的内容：设计变量、基础排版、语言选择基础设施、
//! 无领域语义的展示组件，以及通用交互原语（弹窗外壳、两阶段确认命令状态机）。任何
//! 面向公开竞价或管理命令的组件都必须留在各自的 surface crate，避免 Admin 专属代码
//! 进入公开端 WASM 产物。
//!
//! 本 crate 不得依赖 `auction-storage`、`auction-config` 或任何凭据访问路径。

mod a11y;
mod bootstrap;
mod clock;
mod commands;
mod components;
mod format;
pub mod i18n;
mod media;

pub use i18n::{
    LanguagePicker, SUPPORTED_LOCALES, SupportedLocale, format_localized_time, use_locale_effects,
};

pub use a11y::{use_dialog_focus_trap, use_route_focus};
pub use bootstrap::use_app_boot_ready;
pub use clock::{SecondTicker, use_second_tick, use_second_ticker};
pub use commands::{ConfirmCommand, use_confirm_command};
pub use media::use_media_query;

pub use components::{
    AsyncState, AsyncStateKind, BASE_CSS, BaseStylesheet, Dialog, MarkdownBlocks, MarkdownDocument,
    Pager, Time, TruncatedText, without_leading_title,
};
pub use format::format_credit_amount;
