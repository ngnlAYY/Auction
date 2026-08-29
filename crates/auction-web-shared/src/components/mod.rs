mod markdown;
mod truncated_text;

use dioxus::prelude::*;

pub use markdown::{MarkdownBlocks, MarkdownDocument, without_leading_title};
pub use truncated_text::TruncatedText;

/// 设计变量与基础排版。
///
/// asset! 的绝对路径按声明所在 crate 的包根解析，因此该样式随本 crate 自包含分发；
/// 依赖它的二进制在打包时会自动收集，无需各 surface 重复声明。`static_head` 让 Web
/// 构建把最终带 hash 的 CSS 直接写入 HTML 头部，避免等 WASM 启动后才插入样式表。
pub const BASE_CSS: Asset = asset!("/style/base.css", AssetOptions::css().with_static_head(true));

/// 公开端与管理端共用的品牌图标。
pub const FAVICON: Asset = asset!("/assets/favicon.svg");

/// 渲染共享的基础样式表。
///
/// 各 surface 的根组件应先渲染该组件，再渲染自己的 surface 样式，保证设计变量在
/// surface 规则之前生效。
#[component]
pub fn BaseStylesheet() -> Element {
    // favicon 由两个 surface 的静态 HTML 通过稳定 `/favicon.ico` 路径声明；这里保留 asset
    // 引用，使构建产物继续携带供 HTTP 映射使用的同一份带 hash SVG。
    let _ = FAVICON;
    rsx! {
        document::Stylesheet { href: BASE_CSS }
    }
}
