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

        // panic 后不能由另一条已排队的就绪 effect 重新揭示损坏的组件树。
        if root.has_attribute("data-app-failed") {
            return;
        }
        let _ = root.set_attribute("data-app-ready", "true");
        let _ = root.set_attribute("aria-busy", "false");
    }));
}

/// 安装 WASM panic 兜底钩子。
/// 必须在 Dioxus 启动前调用。panic 后虚拟 DOM 已不可信，不能指望框架渲染错误界面，
/// 因此直接给 `#main` 写入不可逆的失败标记并移除就绪标记，显示静态错误横幅并隐藏损坏的
/// 组件树。等待超过 15 秒只显示加载较慢提示，不再被误报为网络故障；错误横幅仅由 panic
/// 标记触发。两端 index.html 提供站点默认语言的文案，控制台保留原始 panic 信息便于诊断。
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

    // 失败状态优先于所有后续就绪回调；刷新页面后才会重新建立干净的根节点。
    if let Some(root) = document.get_element_by_id("main") {
        let _ = root.set_attribute("data-app-failed", "true");
        let _ = root.remove_attribute("data-app-ready");
        let _ = root.set_attribute("aria-busy", "false");
    }
}
