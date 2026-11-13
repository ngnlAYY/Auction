//! Admin 浏览器应用入口。

mod api;
mod surface;

fn main() {
    // 在 Dioxus 启动前安装 WASM panic 兜底钩子：panic 后向用户展示静态错误横幅。
    auction_web_shared::install_panic_hook();
    surface::run();
}
