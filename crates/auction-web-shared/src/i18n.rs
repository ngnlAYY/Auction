//! 语言选择基础设施。
//!
//! 语言集合、持久化键和 `<html lang>` 同步行为沿用旧版 Auction-BF 的
//! `src/shared/i18n/createI18n.ts` 契约，保证用户在新旧系统间切换时语言偏好不丢失：
//!
//! - `localStorage` 键固定为 `hv.lang`；
//! - 存储值必须在受支持集合内，否则回退到该 surface 的默认语言；
//! - 语言变化时同步 `document.documentElement.lang`。
//!
//! 各 surface 自己拥有译文资源与默认语言：公开端默认英文，管理端默认简体中文。
//! 本模块只提供与业务无关的语言选择能力，不含任何 surface 文案。

use dioxus::prelude::*;
use dioxus_i18n::prelude::*;
use unic_langid::{LanguageIdentifier, langid};

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

/// 受支持语言集合，顺序与旧系统语言切换器一致。
///
/// 中文按脚本区分简繁，两者不能互相回退。
pub const SUPPORTED_LOCALES: &[SupportedLocale] = &[
    SupportedLocale { code: "en", label: "English" },
    SupportedLocale { code: "zh-CN", label: "简体中文" },
    SupportedLocale { code: "zh-TW", label: "繁體中文" },
    SupportedLocale { code: "ja", label: "日本語" },
    SupportedLocale { code: "ko", label: "한국어" },
];

/// 解析语言标记。
///
/// 只接受 `SUPPORTED_LOCALES` 中的取值；未知或损坏的输入返回 `None`，由调用方决定回退。
pub fn parse_supported(code: &str) -> Option<LanguageIdentifier> {
    if !SUPPORTED_LOCALES.iter().any(|locale| locale.code == code) {
        return None;
    }

    code.parse().ok()
}

/// 读取浏览器中已保存的语言偏好。
///
/// 存储不可用（隐私模式、被策略禁用）时返回 `None` 而不是让界面崩溃。
pub fn stored_language() -> Option<LanguageIdentifier> {
    let stored = local_storage()?.get_item(LANGUAGE_STORAGE_KEY).ok()??;

    parse_supported(&stored)
}

/// 决定初始语言：优先使用已保存偏好，否则用该 surface 的默认语言。
pub fn initial_language(default_locale: LanguageIdentifier) -> LanguageIdentifier {
    stored_language().unwrap_or(default_locale)
}

/// 切换当前语言，并持久化到 `localStorage`。
///
/// 写入失败只影响下次启动时的偏好恢复，不影响本次会话内的切换结果，因此忽略错误。
pub fn set_language(i18n: &mut I18n, code: &str) {
    let Some(language) = parse_supported(code) else {
        return;
    };

    i18n.set_language(language.clone());

    if let Some(storage) = local_storage() {
        let _ = storage.set_item(LANGUAGE_STORAGE_KEY, code);
    }
}

/// 把当前语言同步到 `<html lang>`。
///
/// 该属性影响屏幕阅读器发音、浏览器翻译提示和 CSS 的 `:lang()` 选择器，必须随语言变化更新。
pub fn sync_document_language(language: &LanguageIdentifier) {
    if let Some(document) = web_sys::window().and_then(|window| window.document())
        && let Some(element) = document.document_element()
    {
        let _ = element.set_attribute("lang", &language.to_string());
    }
}

/// 在语言变化时同步 `<html lang>`。
///
/// 各 surface 的根组件在 `use_init_i18n` 之后调用一次即可。
pub fn use_locale_effects(i18n: I18n) {
    use_effect(move || {
        sync_document_language(&i18n.language());
    });
}

/// 语言切换器。
///
/// 使用原生 `<select>`：它自带键盘操作、屏幕阅读器语义和移动端原生选择面板，
/// 不需要额外的可访问性实现。
#[component]
pub fn LanguagePicker(label: String) -> Element {
    let mut i18n = i18n();
    let current = i18n.language().to_string();

    rsx! {
        label { class: "language-picker",
            span { class: "language-picker__label", "{label}" }
            select {
                class: "language-picker__select",
                value: "{current}",
                onchange: move |event| set_language(&mut i18n, &event.value()),
                for locale in SUPPORTED_LOCALES {
                    option { value: locale.code, "{locale.label}" }
                }
            }
        }
    }
}

/// 默认语言：公开端使用英文。
pub const DEFAULT_PUBLIC_LOCALE: LanguageIdentifier = langid!("en");

/// 默认语言：管理端沿用旧 `manifests/admin.locales.json` 的简体中文。
pub const DEFAULT_ADMIN_LOCALE: LanguageIdentifier = langid!("zh-CN");

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}
