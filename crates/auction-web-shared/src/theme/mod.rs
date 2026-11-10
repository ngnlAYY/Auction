//! 界面主题基础设施：深色（默认，保持既有视觉契约）与浅色两套设计 token。
//! 偏好键固定为 `hv.theme`，只接受 `dark` / `light`，缺失、非法或存储不可用时使用深色；
//! 主题经 `<html data-theme>` 生效，配色全部由 base.css 与各 surface 的 token 覆盖决定，组件不写颜色。
//! 静态启动骨架无法在 WASM 就绪前读取偏好（CSP 禁止内联脚本），因此始终以深色显示。

use dioxus::prelude::*;

/// 主题偏好的 `localStorage` 键；Public 与 Admin 同源共享，不得重命名。
pub const THEME_STORAGE_KEY: &str = "hv.theme";

/// 受支持的界面主题。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

impl Theme {
    /// 存储值与 `<html data-theme>` 属性值。
    pub const fn code(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    /// 只接受闭集取值，调用方对 `None` 回退默认主题。
    pub fn parse(code: &str) -> Option<Self> {
        match code {
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            _ => None,
        }
    }

    /// 浏览器界面色（`<meta name="theme-color">`），与 base.css 中对应主题的 `--color-bg` 一致。
    const fn browser_color(self) -> &'static str {
        match self {
            Self::Dark => "#0e1116",
            Self::Light => "#f6f8fa",
        }
    }
}

/// 根组件提供的当前主题；`ThemePicker` 通过上下文读写，避免各页面各自持有状态。
#[derive(Clone, Copy)]
struct ThemeState(Signal<Theme>);

/// 读取已保存的主题偏好；存储不可用或值非法时返回 `None`。
fn stored_theme() -> Option<Theme> {
    crate::storage::get_item(THEME_STORAGE_KEY).as_deref().and_then(Theme::parse)
}

/// 把主题同步到 `<html data-theme>` 与浏览器界面色。
fn apply_theme(theme: Theme) {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    if let Some(root) = document.document_element() {
        let _ = root.set_attribute("data-theme", theme.code());
    }
    if let Ok(Some(meta)) = document.query_selector("meta[name=\"theme-color\"]") {
        let _ = meta.set_attribute("content", theme.browser_color());
    }
}

/// 各 surface 根组件调用一次：恢复已保存主题并提供上下文，主题变化时同步 DOM。
/// 首次渲染即写入属性，应用显示前已是目标主题，不会先闪现默认配色。
pub fn use_theme_effects() {
    let theme = use_context_provider(|| {
        let theme = stored_theme().unwrap_or_default();
        apply_theme(theme);
        ThemeState(Signal::new(theme))
    })
    .0;
    use_effect(move || apply_theme(theme()));
}

/// 主题切换器。与语言切换器相同使用原生 `<select>`，自带键盘操作与屏幕阅读器语义；
/// 文案由调用方的 surface 译文提供。写入偏好失败只影响下次启动恢复，不影响本次切换。
#[component]
pub fn ThemePicker(label: String, dark_label: String, light_label: String) -> Element {
    let mut theme = use_context::<ThemeState>().0;
    let current = theme();

    rsx! {
        label { class: "theme-picker",
            span { class: "theme-picker__label", "{label}" }
            select {
                id: "theme",
                name: "theme",
                class: "theme-picker__select",
                value: current.code(),
                onchange: move |event| {
                    let selected = Theme::parse(&event.value()).unwrap_or_default();
                    theme.set(selected);
                    let _ = crate::storage::set_item(THEME_STORAGE_KEY, selected.code());
                },
                // 与语言切换器相同逐项声明 selected，刷新后下拉值与 hv.theme 保持一致。
                option { value: Theme::Dark.code(), selected: current == Theme::Dark, "{dark_label}" }
                option { value: Theme::Light.code(), selected: current == Theme::Light, "{light_label}" }
            }
        }
    }
}
