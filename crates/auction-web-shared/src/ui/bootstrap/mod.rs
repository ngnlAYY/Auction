//! Public 与 Admin 共用的首屏启动门禁与浏览器错误信标。

#[cfg(target_arch = "wasm32")]
use dioxus::prelude::*;

/// Public/Admin 各自暴露的匿名信标路径；路由契约见 `docs/contracts/routes.md`。
pub const BROWSER_ERROR_ROUTE: &str = "/api/v1/browser-errors";

/// 浏览器构建号；发布构建通过 `AUCTION_BUILD_ID` 注入，未注入时退回 workspace 版本。
/// 服务端用同一表达式比较构建号，只把偏差计入固定计数，不把值写入指标标签。
const INJECTED_BUILD_ID: &str = match option_env!("AUCTION_BUILD_ID") {
    Some(value) => value,
    None => env!("CARGO_PKG_VERSION"),
};

static BUILD_ID: std::sync::LazyLock<&'static str> = std::sync::LazyLock::new(|| {
    if build_id_is_valid(INJECTED_BUILD_ID) { INJECTED_BUILD_ID } else { env!("CARGO_PKG_VERSION") }
});

/// 当前页面构建号；非法注入回退包版本，保证比较与上报使用同一份可判定值。
pub fn build_id() -> &'static str {
    *BUILD_ID
}

/// 信标只接受闭集类别；WASM panic、DTO 解码失败与实时协议错误之外的错误不进入该通道。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserErrorClass {
    WasmPanic,
    DtoDecode,
    RealtimeProtocol,
}

impl BrowserErrorClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WasmPanic => "wasm_panic",
            Self::DtoDecode => "dto_decode",
            Self::RealtimeProtocol => "realtime_protocol",
        }
    }
}

/// 构建号只允许 1–64 个 URL 安全字符；非法时客户端降级为 `unknown`，服务端仍会拒绝任意违规值。
pub fn build_id_is_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// 以当前构建号发送一条信标；请求无正文，失败静默，不影响页面主流程。
pub fn report_browser_error(class: BrowserErrorClass) {
    report_browser_error_with_build_id(class, build_id());
}

/// 供测试或显式构建号调用；非法构建号降级为 `unknown`，避免把任意文本拼进 URL。
pub fn report_browser_error_with_build_id(class: BrowserErrorClass, build_id: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        let build_id = if build_id_is_valid(build_id) { build_id } else { "unknown" };
        let url = format!("{BROWSER_ERROR_ROUTE}?class={}&build={build_id}", class.as_str());
        js_sys::futures::spawn_local(async move {
            let _ = gloo_net::http::Request::post(&url).send().await;
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (class, build_id);
    }
}

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
    {
        use std::sync::atomic::{AtomicBool, Ordering};

        // panic 期间新分配或 fetch 再次 panic 时不能递归进入同一个钩子。
        static REPORTED: AtomicBool = AtomicBool::new(false);
        std::panic::set_hook(Box::new(|info| {
            web_sys::console::error_1(&wasm_bindgen::JsValue::from_str(&format!(
                "WASM panic: {info}"
            )));
            show_panic_banner();
            if !REPORTED.swap(true, Ordering::Relaxed) {
                report_browser_error(BrowserErrorClass::WasmPanic);
            }
        }));
    }
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

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::bootstrap as parent;

    #[test]
    fn classes_are_a_closed_wire_set() {
        assert_eq!(BrowserErrorClass::WasmPanic.as_str(), "wasm_panic");
        assert_eq!(BrowserErrorClass::DtoDecode.as_str(), "dto_decode");
        assert_eq!(BrowserErrorClass::RealtimeProtocol.as_str(), "realtime_protocol");
    }

    #[test]
    fn build_id_accepts_only_url_safe_bounded_values() {
        assert!(build_id_is_valid("0.1.0"));
        assert!(build_id_is_valid("build_2026-11-04.abc"));
        assert!(!build_id_is_valid(""));
        assert!(!build_id_is_valid("two words"));
        assert!(!build_id_is_valid("a/b"));
        assert!(!build_id_is_valid(&"a".repeat(65)));
    }

    #[test]
    fn non_wasm_reporter_is_a_noop() {
        report_browser_error(BrowserErrorClass::WasmPanic);
        report_browser_error_with_build_id(BrowserErrorClass::DtoDecode, "invalid build id");
    }
}
