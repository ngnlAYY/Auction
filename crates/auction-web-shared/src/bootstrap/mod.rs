//! Public 与 Admin 共用的首屏启动门禁。

#[cfg(target_arch = "wasm32")]
use dioxus::prelude::*;

/// 必要的首屏请求完成后，仅向宿主节点写入一次就绪标记。
/// 请求失败也由调用方视为“已完成”，让真实错误界面替代静态骨架；后续刷新、登出或路由
/// 变化不再移除该标记，避免 SPA 生命周期内再次遮住可操作界面。
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

        // panic 后不能由另一条已排队的就绪 effect 重新揭示损坏的组件树。
        if root.has_attribute("data-app-failed") {
            return;
        }
        let _ = root.set_attribute("data-app-ready", "true");
        let _ = root.set_attribute("aria-busy", "false");
    }));
}

/// 安装 WASM panic 兜底钩子；必须在 Dioxus 启动前调用。
/// panic 后虚拟 DOM 已不可信，直接给 `#main` 写入不可逆的失败标记并移除就绪标记，显示
/// index.html 的静态错误横幅。宿主端为空操作，保证非 wasm32 目标可编译。
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

    // 失败状态优先于所有后续就绪回调；刷新页面后才会重新建立干净的根节点。
    if let Some(root) = document.get_element_by_id("main") {
        let _ = root.set_attribute("data-app-failed", "true");
        let _ = root.remove_attribute("data-app-ready");
        let _ = root.set_attribute("aria-busy", "false");
    }
}
