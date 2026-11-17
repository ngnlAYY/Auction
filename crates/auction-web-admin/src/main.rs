//! Admin 浏览器应用入口。

// 测试里的断言直接解包；该豁免只在测试构建生效，生产构建仍按 workspace lint 拒绝 unwrap/expect。
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod api;
mod surface;

fn main() {
    // 在 Dioxus 启动前安装 WASM panic 兜底钩子：panic 后向用户展示静态错误横幅。
    auction_web_shared::install_panic_hook();
    surface::run();
}
