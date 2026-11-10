//! 语言选择基础设施。
//! 语言集合、持久化键和 `<html lang>` 同步沿用旧版契约，保证用户在新旧系统间切换时
//! 语言偏好不丢失：`localStorage` 键固定为 `hv.lang`，存储值非法时回退到该 surface 的
//! 默认语言。各 surface 自己拥有译文资源与默认语言（Public 英文、Admin 简体中文）。

/// 按旧版 i18next 约定翻译消息，不保留 Fluent 自动添加的双向文本隔离外框。
/// `dioxus-i18n 0.5.1` 无公开 `set_use_isolating` 入口，默认会在每个动态变量外插入
/// U+2068/U+2069；这里移除最外层隔离对，变量内部的嵌套对仍保留。
#[macro_export]
macro_rules! legacy_t {
    ($id:expr, $( $name:ident : $value:expr ),* ) => {{
        let translated = dioxus_i18n::te!($id, $( $name : $value ),*)
            .unwrap_or_else(|error| panic!("{}", error.to_string()));
        $crate::i18n::without_fluent_isolation(translated)
    }};

    ($id:expr) => {{
        let translated = dioxus_i18n::te!($id)
            .unwrap_or_else(|error| panic!("{}", error.to_string()));
        $crate::i18n::without_fluent_isolation(translated)
    }};
}

use dioxus::prelude::*;
use dioxus_i18n::prelude::*;
use unic_langid::{LanguageIdentifier, langid};

/// 移除 Fluent 自动包裹动态变量的最外层 FSI/PDI，同时保留变量内部成对的隔离控制符。
#[doc(hidden)]
pub fn without_fluent_isolation(mut value: String) -> String {
    if !value.contains(['\u{2068}', '\u{2069}']) {
        return value;
    }

    // 译文已经拥有独立缓冲区，原地保留字符，避免每次动态翻译再分配并复制整段文本。
    let mut isolation_depth = 0_u32;
    value.retain(|character| match character {
        '\u{2068}' if isolation_depth == 0 => {
            isolation_depth = 1;
            false
        }
        '\u{2068}' => {
            isolation_depth += 1;
            true
        }
        '\u{2069}' if isolation_depth == 1 => {
            isolation_depth = 0;
            false
        }
        '\u{2069}' if isolation_depth > 1 => {
            isolation_depth -= 1;
            true
        }
        _ => true,
    });
    value
}

/// 按显式语言把 RFC3339 时间显示为 Asia/Shanghai 时间，不读取 DOM 或依赖语言同步 effect。
/// 浏览器复用最近语言的 `Intl.DateTimeFormat`；非 WASM 构建保留同一目标时区的兜底格式。
pub fn format_localized_time_for_language(value: &str, _language: &str) -> String {
    if value.is_empty() {
        return "—".to_owned();
    }

    #[cfg(target_arch = "wasm32")]
    {
        let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(value));
        if date.get_time().is_finite() {
            let formatter = time_formatter(_language);
            if let Ok(formatted) =
                formatter.format().call1(&wasm_bindgen::JsValue::NULL, date.as_ref())
                && let Some(formatted) = formatted.as_string()
            {
                return formatted;
            }
        }
    }

    value
        .parse::<chrono::DateTime<chrono::FixedOffset>>()
        .ok()
        .and_then(|instant| {
            chrono::FixedOffset::east_opt(8 * 60 * 60).map(|offset| instant.with_timezone(&offset))
        })
        .map(|instant| instant.format("%d %b %Y, %H:%M:%S").to_string())
        .unwrap_or_else(|| "—".to_owned())
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    // 两端同一渲染树使用一种语言；单槽缓存避免逐时间单元格构造 Intl，也不随语言输入无限增长。
    static TIME_FORMATTER: std::cell::RefCell<Option<(String, js_sys::Intl::DateTimeFormat)>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(target_arch = "wasm32")]
fn time_formatter(language: &str) -> js_sys::Intl::DateTimeFormat {
    TIME_FORMATTER.with_borrow_mut(|cached| {
        if let Some((cached_language, formatter)) = cached.as_ref()
            && cached_language == language
        {
            return formatter.clone();
        }
        let locales = js_sys::Array::of1(&wasm_bindgen::JsValue::from_str(language));
        let options = js_sys::Object::new();
        for (name, option) in [
            ("day", "numeric"),
            ("hour", "2-digit"),
            ("minute", "2-digit"),
            ("month", "short"),
            ("second", "2-digit"),
            ("year", "numeric"),
        ] {
            let _ = js_sys::Reflect::set(
                &options,
                &wasm_bindgen::JsValue::from_str(name),
                &wasm_bindgen::JsValue::from_str(option),
            );
        }
        let _ = js_sys::Reflect::set(
            &options,
            &wasm_bindgen::JsValue::from_str("hour12"),
            &wasm_bindgen::JsValue::FALSE,
        );
        let _ = js_sys::Reflect::set(
            &options,
            &wasm_bindgen::JsValue::from_str("timeZone"),
            &wasm_bindgen::JsValue::from_str("Asia/Shanghai"),
        );
        let formatter = js_sys::Intl::DateTimeFormat::new(&locales, &options);
        *cached = Some((language.to_owned(), formatter.clone()));
        formatter
    })
}

/// 旧系统的语言偏好存储键。跨系统共享该键，不得重命名。
pub const LANGUAGE_STORAGE_KEY: &str = "hv.lang";

/// 一个受支持的界面语言。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportedLocale {
    /// BCP 47 语言标记，同时用作 `localStorage` 取值和 `<html lang>` 属性值。
    pub code: &'static str,
    /// 语言切换器中显示的名称，按惯例使用该语言自身的写法。
    pub label: &'static str,
}

/// 受支持语言集合，顺序与旧系统语言切换器一致。中文按脚本区分简繁，不能互相回退。
pub const SUPPORTED_LOCALES: &[SupportedLocale] = &[
    SupportedLocale { code: "en", label: "English" },
    SupportedLocale { code: "zh-CN", label: "简体中文" },
    SupportedLocale { code: "zh-TW", label: "繁體中文" },
    SupportedLocale { code: "ja", label: "日本語" },
    SupportedLocale { code: "ko", label: "한국어" },
];

/// 解析语言标记；只接受 `SUPPORTED_LOCALES` 中的取值，否则返回 `None` 由调用方回退。
pub fn parse_supported(code: &str) -> Option<LanguageIdentifier> {
    if !SUPPORTED_LOCALES.iter().any(|locale| locale.code == code) {
        return None;
    }

    code.parse().ok()
}

/// 读取浏览器已保存的语言偏好；存储不可用时返回 `None` 而不是让界面崩溃。
pub fn stored_language() -> Option<LanguageIdentifier> {
    let stored = crate::storage::get_item(LANGUAGE_STORAGE_KEY)?;

    parse_supported(&stored)
}

/// 决定初始语言：优先使用已保存偏好，否则用该 surface 的默认语言。
pub fn initial_language(default_locale: LanguageIdentifier) -> LanguageIdentifier {
    stored_language().unwrap_or(default_locale)
}

/// 切换当前语言并持久化到 `localStorage`。非法语言码回退并持久化 surface 默认语言；
/// 写入失败只影响下次启动的偏好恢复，不影响本次会话内切换，因此忽略错误。
pub fn set_language(i18n: &mut I18n, code: &str, default_locale: &'static str) {
    let resolved = if parse_supported(code).is_some() { code } else { default_locale };
    let Some(language) = parse_supported(resolved) else {
        return;
    };

    i18n.set_language(language.clone());

    let _ = crate::storage::set_item(LANGUAGE_STORAGE_KEY, resolved);
}

/// 把当前语言同步到 `<html lang>`。
/// 该属性影响屏幕阅读器发音、浏览器翻译提示与 CSS `:lang()`，必须随语言变化更新。
pub fn sync_document_language(language: &LanguageIdentifier) {
    if let Some(document) = web_sys::window().and_then(|window| window.document())
        && let Some(element) = document.document_element()
    {
        let _ = element.set_attribute("lang", &language.to_string());
    }
}

/// 在语言变化时同步 `<html lang>`；各 surface 根组件在 `use_init_i18n` 之后调用一次。
pub fn use_locale_effects(i18n: I18n) {
    use_effect(move || {
        sync_document_language(&i18n.language());
    });
}

/// 语言切换器。用原生 `<select>` 自带键盘操作、屏幕阅读器语义和移动端选择面板。
#[component]
pub fn LanguagePicker(label: String, default_locale: &'static str) -> Element {
    let mut i18n = i18n();
    let current = i18n.language().to_string();

    rsx! {
        label { class: "language-picker",
            span { class: "language-picker__label", "{label}" }
            select {
                id: "language",
                name: "language",
                class: "language-picker__select",
                value: "{current}",
                onchange: move |event| {
                    set_language(&mut i18n, &event.value(), default_locale)
                },
                for locale in SUPPORTED_LOCALES {
                    // Dioxus 首轮挂载动态 option 时不会可靠地把 select.value 投影为原生
                    // selected；逐项声明可确保刷新后下拉值与 hv.lang、<html lang> 一致。
                    option {
                        value: locale.code,
                        selected: current == locale.code,
                        "{locale.label}"
                    }
                }
            }
        }
    }
}

/// 默认语言：公开端使用英文。
pub const DEFAULT_PUBLIC_LOCALE: LanguageIdentifier = langid!("en");

/// 默认语言：管理端沿用旧 `manifests/admin.locales.json` 的简体中文。
pub const DEFAULT_ADMIN_LOCALE: LanguageIdentifier = langid!("zh-CN");
