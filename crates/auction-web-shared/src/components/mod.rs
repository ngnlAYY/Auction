use dioxus::prelude::*;

/// 设计变量与基础排版。
///
/// asset! 的绝对路径按声明所在 crate 的包根解析，因此该样式随本 crate 自包含分发；
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
