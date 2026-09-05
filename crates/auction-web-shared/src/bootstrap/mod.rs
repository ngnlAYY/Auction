//! Public 与 Admin 共用的首屏启动门禁。

#[cfg(target_arch = "wasm32")]
use dioxus::prelude::*;

/// 必要的首屏请求完成后，仅向宿主节点写入一次就绪标记。
///
/// 请求失败也应由调用方视为“已完成”，让真实错误界面能够替代静态骨架；后续刷新、登出或
/// 路由变化不会移除该标记，避免 SPA 生命周期内再次遮住可操作界面。
pub fn use_app_boot_ready(_ready: bool) {
    #[cfg(target_arch = "wasm32")]
    use_effect(use_reactive((&_ready,), move |(ready,)| {
        if !ready {
            return;
        }

        let Some(root) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id("main"))
        else {
            return;
        };

        let _ = root.set_attribute("data-app-ready", "true");
        let _ = root.set_attribute("aria-busy", "false");
    }));
}

/// 安装 WASM panic 兜底钩子。
/// 必须在 Dioxus 启动前调用。panic 后虚拟 DOM 已不可信，不能指望框架渲染错误界面，
/// 因此直接操作 index.html 骨架屏：移除 `#main` 的 `data-app-ready` 让 `.app-boot` 骨架
/// 重新可见（同时隐藏已损坏的 `.app-root`），再把骨架内的静态错误横幅 `.app-boot__error`
/// 的 15 秒延迟动画取消，使其立即显现。横幅文案由两端 index.html 静态提供（已按站点语言
/// 本地化），无需任何脚本注入。控制台同时保留原始 panic 信息便于诊断。
/// 宿主端为空操作，保证非 wasm32 目标（如 `cargo check`）可编译。
pub fn install_panic_hook() {
    #[cfg(target_arch = "wasm32")]
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&wasm_bindgen::JsValue::from_str(&format!("WASM panic: {info}")));
        show_panic_banner();
    }));
}

/// 把 index.html 骨架屏中的静态错误横幅切换为立即可见。
#[cfg(target_arch = "wasm32")]
fn show_panic_banner() {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };

    // 撤掉就绪标记：CSS 会重新显示 .app-boot 骨架并隐藏 .app-root。
    if let Some(root) = document.get_element_by_id("main") {
        let _ = root.remove_attribute("data-app-ready");
        let _ = root.set_attribute("aria-busy", "false");
    }

    // 骨架容器带 aria-hidden="true"，横幅可见时必须解除，否则屏幕阅读器读不到错误。
    if let Some(boot) = document.query_selector(".app-boot").ok().flatten() {
        let _ = boot.set_attribute("aria-hidden", "false");
    }

    // 取消横幅的延迟动画，立即显示。
    if let Some(banner) = document.query_selector(".app-boot__error").ok().flatten() {
        let _ = banner.set_attribute("style", "animation: none; opacity: 1; visibility: visible;");
    }
}
