//! Public 与 Admin 共用的浏览器端基础能力。
//!
//! 只允许放真正通用、没有业务偏向的内容：设计变量、基础排版、语言选择基础设施和无领域
//! 语义的展示组件。任何面向公开竞价或管理命令的组件都必须留在各自的 surface crate，
//! 避免 Admin 专属代码进入公开端 WASM 产物。
//!
//! 本 crate 不得依赖 `auction-storage`、`auction-config` 或任何凭据访问路径。

pub mod i18n;

use dioxus::prelude::*;

pub use i18n::{LanguagePicker, SUPPORTED_LOCALES, SupportedLocale, use_locale_effects};

/// 设计变量与基础排版。
///
/// `asset!` 的绝对路径按声明所在 crate 的包根解析，因此该样式随本 crate 自包含分发；
/// 依赖它的二进制在打包时会自动收集，无需各 surface 重复声明。
pub const BASE_CSS: Asset = asset!("/style/base.css");

/// 渲染共享的基础样式表。
///
/// 各 surface 的根组件应先渲染该组件，再渲染自己的 surface 样式，保证设计变量在
/// surface 规则之前生效。
#[component]
pub fn BaseStylesheet() -> Element {
    rsx! {
        document::Stylesheet { href: BASE_CSS }
    }
}
