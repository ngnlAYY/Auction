//! Public 与 Admin 共用的浏览器端基础能力。
//!
//! 只允许放真正通用、没有业务偏向的内容：设计变量、基础排版、语言选择基础设施和无领域
//! 语义的展示组件。任何面向公开竞价或管理命令的组件都必须留在各自的 surface crate，
//! 避免 Admin 专属代码进入公开端 WASM 产物。
//!
//! 本 crate 不得依赖 `auction-storage`、`auction-config` 或任何凭据访问路径。

mod components;
pub mod i18n;

pub use i18n::{LanguagePicker, SUPPORTED_LOCALES, SupportedLocale, use_locale_effects};

pub use components::{BASE_CSS, BaseStylesheet};
