//! 两端共用的 UI 组件与共享样式资源。

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

/// 设计变量、基础排版与通用组件的手机默认值。
/// `asset!` 绝对路径按声明所在 crate 的包根解析，样式随本 crate 自包含分发；`with_static_head` 让
/// Web 构建把带 hash 的 CSS 直接写入 HTML 头部，避免等 WASM 启动后才插入样式表。
const BASE_CSS: Asset = asset!("/style/base.css", AssetOptions::css().with_static_head(true));

/// 两端共用的控制台外壳样式；单端专属规则留在各 surface 自己的样式文件。
const CONSOLE_CSS: Asset = asset!("/style/console.css", AssetOptions::css().with_static_head(true));

/// 公开端与管理端共用的品牌图标。
pub const FAVICON: Asset = asset!("/assets/favicon.svg");

/// 安装两端共用的表格提示与 VisualViewport 生命周期，并让共享样式与图标进入构建产物。
/// 样式表只由静态 head 声明一次：运行时再插入同址样式表会让此后每次样式重算多匹配一遍全部规则。
/// 同层规则的先后（base → surface → desktop → mobile → console）由 `ci/release/web-bundle.sh` 固定。
#[component]
pub fn BaseStylesheet() -> Element {
    table_hints::use_table_hints();
    crate::media::use_visual_viewport();
    // asset 常量只有被引用才进入构建产物：样式由 dx 写入静态 head，favicon 由两个 surface 的静态
    // HTML 经稳定 `/favicon.ico` 路径声明，HTTP 映射仍需同一份带 hash 的 SVG。
    let _ = (BASE_CSS, CONSOLE_CSS, FAVICON);
    rsx! {}
}
