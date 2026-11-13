//! 两端共用的 UI 组件与样式表入口。

mod async_state;
mod dialog;
mod markdown;
mod pager;
mod table_columns;
mod table_hints;
mod time;
mod truncated_text;

use dioxus::prelude::*;

pub use async_state::{AsyncState, AsyncStateKind};
pub use dialog::Dialog;
pub use markdown::{MarkdownBlocks, MarkdownDocument, without_leading_title};
pub use pager::Pager;
pub use table_columns::TableColumns;
pub use time::Time;
pub use truncated_text::TruncatedText;

/// 设计变量与基础排版。
/// `asset!` 绝对路径按声明所在 crate 的包根解析，样式随本 crate 自包含分发；`static_head` 让
/// Web 构建把带 hash 的 CSS 直接写入 HTML 头部，避免等 WASM 启动后才插入样式表。
pub const BASE_CSS: Asset = asset!("/style/base.css", AssetOptions::css().with_static_head(true));

/// 两端共用的控制台外壳样式；单端专属规则留在各 surface 自己的样式文件。
pub const CONSOLE_CSS: Asset =
    asset!("/style/console.css", AssetOptions::css().with_static_head(true));

const MOBILE_CSS: Asset = asset!("/style/mobile.css", AssetOptions::css().with_static_head(true));

/// 公开端与管理端共用的品牌图标。
pub const FAVICON: Asset = asset!("/assets/favicon.svg");

/// 渲染共享的基础样式表。
/// CSS 层显式固定基础、surface、桌面和移动规则优先级，不依赖 Dioxus 的 head 插入顺序。
#[component]
pub fn BaseStylesheet() -> Element {
    table_hints::use_table_hints();
    crate::media::use_visual_viewport();
    // favicon 由两个 surface 的静态 HTML 通过稳定 `/favicon.ico` 路径声明；这里保留 asset
    // 引用，使构建产物继续携带供 HTTP 映射使用的同一份带 hash SVG。
    let _ = FAVICON;
    rsx! {
        document::Stylesheet { href: BASE_CSS }
        document::Stylesheet { href: MOBILE_CSS }
    }
}

/// 渲染统一控制台外壳样式；必须放在 surface 样式之后，才能覆盖壳层与页面容器。
#[component]
pub fn ConsoleStylesheet() -> Element {
    rsx! {
        document::Stylesheet { href: CONSOLE_CSS }
    }
}
