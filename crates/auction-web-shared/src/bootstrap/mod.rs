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
