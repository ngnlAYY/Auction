//! 语言选择基础设施。
//! 语言集合、持久化键和 `<html lang>` 同步是已发布的浏览器契约，保证升级前后用户已保存的
//! 语言偏好不丢失：`localStorage` 键固定为 `hv.lang`，存储值非法时回退到该 surface 的
//! 默认语言。各 surface 自己拥有译文资源与默认语言（Public 英文、Admin 简体中文）。

use dioxus::dioxus_core::Runtime;
use dioxus::prelude::*;
use dioxus_i18n::prelude::*;
use unic_langid::{LanguageIdentifier, langid};

mod translation;
pub use translation::translate_without_args;

/// 按旧版 i18next 约定翻译消息，不保留 Fluent 自动添加的双向文本隔离外框。
/// `dioxus-i18n` 无公开 `set_use_isolating` 入口，默认会在每个动态变量外插入
/// U+2068/U+2069；这里移除最外层隔离对，变量内部的嵌套对仍保留。
/// 翻译失败交给 [`i18n::translation_failed`](crate::i18n::translation_failed)：开发与测试构建
/// 立即 panic，发布构建退回显示消息 ID，不让一条文案拖垮整个页面。
#[macro_export]
macro_rules! t {
    ($id:expr, $( $name:ident : $value:expr ),* ) => {{
        let id: &str = $id;
        match dioxus_i18n::te!(id, $( $name : $value ),*) {
            Ok(translated) => $crate::i18n::without_fluent_isolation(translated),
            Err(error) => $crate::i18n::translation_failed(id, &error),
        }
    }};

    ($id:expr) => {{
        let id: &str = $id;
        $crate::i18n::translate_without_args(id)
    }};
}

/// 翻译失败（缺键、译文引用了未传入的变量等）的处理。
///
/// 开发与测试构建开启调试断言，直接 panic 让问题在测试里暴露；发布构建把消息 ID 作为显示
/// 文本返回，并在页面生命周期内只上报一次 `i18n_format` 浏览器错误信标。界面文案不决定金额、
/// 资格或命令结果，退回显示 ID 比整页进入 panic 横幅更安全。
#[doc(hidden)]
pub fn translation_failed(id: &str, error: &dyn std::fmt::Display) -> String {
    if cfg!(debug_assertions) {
        panic!("翻译 {id} 失败：{error}");
    }
    static REPORTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !REPORTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        crate::report_browser_error(crate::BrowserErrorClass::I18nFormat);
    }
    id.to_owned()
}

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
        .parse::<auction_time::Timestamp>()
        .ok()
        .and_then(shanghai_fallback_text)
        .unwrap_or_else(|| "—".to_owned())
}

/// 兜底格式 `DD Mon YYYY, HH:MM:SS`：Asia/Shanghai 全年固定 UTC+8，平移八小时后直接取 UTC 字段。
fn shanghai_fallback_text(instant: auction_time::Timestamp) -> Option<String> {
    const MONTHS: [&str; 12] =
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let local = instant.checked_add_signed(auction_time::Duration::hours(8))?.to_utc_datetime();
    let month = MONTHS.get(usize::from(local.month().unsigned_abs()).checked_sub(1)?)?;
    Some(format!(
        "{:02} {month} {:04}, {:02}:{:02}:{:02}",
        local.day(),
        local.year(),
        local.hour(),
        local.minute(),
        local.second()
    ))
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

/// 已发布的语言偏好存储键；用户浏览器里已保存的偏好依赖该键，不得重命名。
pub const LANGUAGE_STORAGE_KEY: &str = "hv.lang";

/// 一个受支持的界面语言。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportedLocale {
    /// BCP 47 语言标记，同时用作 `localStorage` 取值和 `<html lang>` 属性值。
    pub code: &'static str,
    /// 语言切换器中显示的名称，按惯例使用该语言自身的写法。
    pub label: &'static str,
}

/// 受支持语言集合，顺序即语言切换器的显示顺序。中文按脚本区分简繁，不能互相回退。
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

/// 一个非默认语言及其带指纹的编译期资源；由各 surface 用 `asset!` 声明。
#[derive(Clone, PartialEq)]
pub struct LocaleSource {
    pub id: LanguageIdentifier,
    pub asset: dioxus::prelude::Asset,
}

/// surface 的语言目录：默认语言编译期内嵌，其余语言按需从同源静态资源取回。
#[derive(Clone, PartialEq)]
pub struct LocaleCatalog {
    pub default_locale: LanguageIdentifier,
    pub default_ftl: &'static str,
    pub sources: &'static [LocaleSource],
}

impl LocaleCatalog {
    fn source(&self, id: &LanguageIdentifier) -> Option<&LocaleSource> {
        self.sources.iter().find(|source| &source.id == id)
    }
}

/// 由默认 FTL 与可选的目标语言 FTL 构造 Fluent bundle；目标缺键时回退默认语言。
pub fn build_bundle(
    default: &LanguageIdentifier,
    default_ftl: &str,
    target: &LanguageIdentifier,
    target_ftl: Option<&str>,
) -> Result<dioxus_i18n::fluent::FluentBundle<dioxus_i18n::fluent::FluentResource>, String> {
    use dioxus_i18n::fluent::{FluentBundle, FluentResource};

    let parse = |text: &str| {
        FluentResource::try_new(text.to_owned()).map_err(|error| format!("{:?}", error.0))
    };
    let mut bundle = FluentBundle::new(vec![target.clone()]);
    bundle.add_resource(parse(default_ftl)?).map_err(|errors| format!("{errors:?}"))?;
    if target != default
        && let Some(text) = target_ftl
    {
        bundle.add_resource_overriding(parse(text)?);
    }
    Ok(bundle)
}

/// 语言会话状态：已生效语言、最近一次选择的代次、在途目标与可见错误。
#[derive(Clone, PartialEq)]
struct LocaleState {
    current: LanguageIdentifier,
    generation: u64,
    pending: Option<LanguageIdentifier>,
    ready: bool,
    error: Option<&'static str>,
}

impl LocaleState {
    fn new(current: LanguageIdentifier) -> Self {
        Self { current, generation: 0, pending: None, ready: false, error: None }
    }

    fn picker_language(&self) -> &LanguageIdentifier {
        self.pending.as_ref().unwrap_or(&self.current)
    }

    fn select(&mut self, target: LanguageIdentifier) -> Option<LocaleRequest> {
        // 即使回选当前语言，也要使此前的加载失去更新译文和偏好的资格。
        self.generation = self.generation.wrapping_add(1);
        self.error = None;
        if self.current == target {
            self.pending = None;
            self.ready = true;
            return None;
        }
        self.pending = Some(target.clone());
        Some(LocaleRequest { target, generation: self.generation })
    }

    fn finish_load(&mut self, request: &LocaleRequest, succeeded: bool) -> bool {
        if request.generation != self.generation {
            return false;
        }
        self.pending = None;
        self.ready = true;
        if succeeded {
            self.current.clone_from(&request.target);
            self.error = None;
        } else {
            self.error = Some("language-switch-failed");
        }
        true
    }
}

struct LocaleRequest {
    target: LanguageIdentifier,
    generation: u64,
}

/// 按需语言会话：持有 bundle 句柄与当前语言信号；根组件创建一次并经 context 提供。
#[derive(Clone, Copy)]
pub struct LocaleSession {
    bundle: Signal<I18nBundle>,
    state: Signal<LocaleState>,
}

impl LocaleSession {
    pub fn new(bundle: I18nBundle, default: LanguageIdentifier) -> Self {
        Self { bundle: Signal::new(bundle), state: Signal::new(LocaleState::new(default)) }
    }

    pub fn current(&self) -> LanguageIdentifier {
        self.state.read().current.clone()
    }

    pub fn is_ready(&self) -> bool {
        self.state.read().ready
    }

    /// 最近一次加载失败的提示键；再次选择语言时清空。
    pub fn error(&self) -> Option<&'static str> {
        self.state.read().error
    }

    /// 首屏加载：默认语言直接就绪；已保存的非默认语言先取回再渲染，失败回退默认。
    pub fn bootstrap(&mut self, catalog: &'static LocaleCatalog) {
        let target = initial_language(catalog.default_locale.clone());
        let request = self.state.write().select(target);
        if let Some(request) = request {
            self.load(catalog, request, false);
        }
    }

    /// 切换语言并持久化；非法语言码回退默认语言。
    pub fn switch(&mut self, catalog: &'static LocaleCatalog, code: &str) {
        let target = match parse_supported(code) {
            Some(language) => language,
            None => catalog.default_locale.clone(),
        };
        let request = self.state.write().select(target.clone());
        if let Some(request) = request {
            self.load(catalog, request, true);
        } else {
            let _ = crate::storage::set_item(LANGUAGE_STORAGE_KEY, &target.to_string());
        }
    }

    fn load(&self, catalog: &'static LocaleCatalog, request: LocaleRequest, persist: bool) {
        let mut session = *self;
        // 在途目标由语言会话收尾，选择器卸载时会话仍能应用结果或清除目标。
        Runtime::current().spawn(self.state.origin_scope(), async move {
            let target = &request.target;
            let fetched = if target == &catalog.default_locale {
                Ok(None)
            } else {
                match catalog.source(target) {
                    Some(source) => fetch_locale_text(source.asset).await.map(Some),
                    None => Err("unsupported locale".to_owned()),
                }
            };
            let built = fetched.and_then(|text| {
                build_bundle(&catalog.default_locale, catalog.default_ftl, target, text.as_deref())
            });
            let mut next = session.state.peek().clone();
            if !next.finish_load(&request, built.is_ok()) {
                return;
            }
            // 代次校验、译文与偏好更新之间没有 await，迟到请求不能部分提交。
            session.state.set(next);
            if let Ok(bundle) = built {
                session.bundle.write().set_bundle(bundle);
                if persist {
                    let _ = crate::storage::set_item(LANGUAGE_STORAGE_KEY, &target.to_string());
                }
            }
        });
    }
}

/// 取回一个带指纹的 FTL 资源；超时或非 2xx 都按失败处理，由调用方保留当前语言。
async fn fetch_locale_text(asset: dioxus::prelude::Asset) -> Result<String, String> {
    use futures_util::future::{Either, select};

    let request = async {
        let response = gloo_net::http::Request::get(&asset.to_string())
            .send()
            .await
            .map_err(|_| "locale fetch failed".to_owned())?;
        if !response.ok() {
            return Err("locale fetch status".to_owned());
        }
        response.text().await.map_err(|_| "locale read failed".to_owned())
    };
    match select(Box::pin(request), Box::pin(crate::timer::TimeoutFuture::new(3000))).await {
        Either::Left((result, _)) => result,
        Either::Right(((), _)) => Err("locale fetch timeout".to_owned()),
    }
}

/// 当前语言；直接读取上下文，不占 hook 槽位，渲染期间仍订阅语言信号。
pub fn current_language() -> LanguageIdentifier {
    consume_context::<LocaleSession>().current()
}

/// 把当前语言同步到 `<html lang>`。
/// 该属性影响屏幕阅读器发音、浏览器翻译提示与 CSS `:lang()`，必须随语言变化更新。
/// 入口文档的 `lang` 只是各 surface 默认语言的静态兜底：CSP 禁止内联脚本，WASM 就绪前
/// 无法按 `hv.lang` 修正，偏好其他语言的用户会有一段短暂的错误语言窗口，属已接受的取舍。
pub fn sync_document_language(_language: &LanguageIdentifier) {
    #[cfg(target_arch = "wasm32")]
    if let Some(document) = web_sys::window().and_then(|window| window.document())
        && let Some(element) = document.document_element()
    {
        let _ = element.set_attribute("lang", &_language.to_string());
    }
}

/// 同步 `<html lang>` 和语言就绪后的启动错误提示；各 surface 根组件调用一次。
pub fn use_locale_effects(session: LocaleSession) {
    use_effect(move || {
        sync_document_language(&session.current());
        #[cfg(any(target_arch = "wasm32", test))]
        if session.is_ready() {
            set_boot_error_text(&crate::t!("boot-error"));
        }
    });
}

#[cfg(any(target_arch = "wasm32", test))]
fn set_boot_error_text(text: &str) {
    // 错误可能发生在组件树之外。只改静态提示的文字，保留入口的显示时机和读屏语义。
    #[cfg(target_arch = "wasm32")]
    if let Some(error) =
        web_sys::window().and_then(|window| window.document()).and_then(|document| {
            document.query_selector("#main > .app-boot > .app-boot__error").ok().flatten()
        })
        && error.text_content().as_deref() != Some(text)
    {
        error.set_text_content(Some(text));
    }
    #[cfg(test)]
    tests::publish_boot_error_text(text);
}

/// 语言切换器。用原生 `<select>` 自带键盘操作、屏幕阅读器语义和移动端选择面板。
#[component]
pub fn LanguagePicker(label: String, catalog: &'static LocaleCatalog) -> Element {
    let mut session = use_context::<LocaleSession>();
    let selected = session.state.read().picker_language().to_string();
    let error = session.error();

    rsx! {
        label { class: "language-picker",
            span { class: "language-picker__label", "{label}" }
            select {
                id: "language",
                name: "language",
                class: "language-picker__select",
                value: "{selected}",
                onchange: move |event| {
                    session.switch(catalog, &event.value());
                },
                for locale in SUPPORTED_LOCALES {
                    // Dioxus 首轮挂载动态 option 时不会可靠地投影 select.value，逐项标记
                    // selected 以跟踪可回选的在途目标或已生效语言。
                    option {
                        value: locale.code,
                        selected: selected == locale.code,
                        "{locale.label}"
                    }
                }
            }
        }
        if error.is_some() {
            p { class: "language-picker__error", role: "alert",
                {crate::t!("language-switch-failed")}
            }
        }
    }
}

/// 默认语言：公开端使用英文。
pub const DEFAULT_PUBLIC_LOCALE: LanguageIdentifier = langid!("en");

/// 默认语言：管理端使用简体中文。
pub const DEFAULT_ADMIN_LOCALE: LanguageIdentifier = langid!("zh-CN");

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use dioxus::dioxus_core::{AttributeValue, Mutation};
    use dioxus::prelude::*;
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;
    use unic_langid::langid;

    use super::translation_failed;
    use super::{
        LanguagePicker, LocaleCatalog, LocaleSession, LocaleState, build_bundle, current_language,
        use_locale_effects,
    };

    #[derive(Default)]
    struct BootBanner {
        session: Cell<Option<LocaleSession>>,
        text: RefCell<Option<String>>,
    }

    pub(super) fn publish_boot_error_text(text: &str) {
        if let Some(banner) = try_consume_context::<Rc<BootBanner>>() {
            banner.text.replace(Some(text.to_owned()));
        }
    }

    static BOOT_CATALOG: LocaleCatalog = LocaleCatalog {
        default_locale: super::DEFAULT_PUBLIC_LOCALE,
        default_ftl: "boot-error = Refresh the page to try again.\n",
        sources: &[],
    };

    fn localized_boot_banner() -> Element {
        let bundle = try_use_init_i18n_from_bundle(|| {
            build_bundle(
                &BOOT_CATALOG.default_locale,
                BOOT_CATALOG.default_ftl,
                &langid!("ja"),
                Some("boot-error = 再読み込みしてください。\n"),
            )
        })
        .unwrap();
        let banner = use_context::<Rc<BootBanner>>();
        let session = use_context_provider(|| LocaleSession::new(bundle, langid!("ja")));
        banner.session.set(Some(session));
        use_locale_effects(session);
        VNode::empty()
    }

    #[test]
    fn boot_banner_keeps_static_text_until_ready_and_retains_the_last_loaded_language() {
        let banner = Rc::new(BootBanner::default());
        let mut dom = VirtualDom::new(localized_boot_banner).with_root_context(banner.clone());
        assert!(banner.text.borrow().is_none());
        dom.rebuild_in_place();
        dom.render_immediate_to_vec();
        assert!(banner.text.borrow().is_none(), "就绪前保留入口 HTML 的提示");
        let mut session = banner.session.get().unwrap();

        dom.in_scope(ScopeId::APP, || session.switch(&BOOT_CATALOG, "ja"));
        dom.render_immediate_to_vec();
        assert_eq!(banner.text.borrow().as_deref(), Some("再読み込みしてください。"));

        dom.in_scope(ScopeId::APP, || session.switch(&BOOT_CATALOG, "ko"));
        dom.render_immediate_to_vec();
        assert_eq!(session.error(), Some("language-switch-failed"));
        assert_eq!(banner.text.borrow().as_deref(), Some("再読み込みしてください。"));

        dom.in_scope(ScopeId::APP, || session.switch(&BOOT_CATALOG, "en"));
        dom.render_immediate_to_vec();
        assert_eq!(session.error(), None);
        assert_eq!(banner.text.borrow().as_deref(), Some("Refresh the page to try again."));
    }

    #[test]
    fn only_the_latest_request_can_finish_when_the_language_repeats() {
        let mut state = LocaleState::new(langid!("en"));
        let old_ja = state.select(langid!("ja")).expect("发起第一次日语读取");
        let ko = state.select(langid!("ko")).expect("发起韩语读取");
        let new_ja = state.select(langid!("ja")).expect("发起第二次日语读取");

        assert!(!state.finish_load(&old_ja, false));
        assert!(!state.finish_load(&ko, true));
        assert_eq!(state.current, langid!("en"));
        assert_eq!(state.picker_language(), &langid!("ja"));
        assert_eq!(state.error, None);
        assert!(state.finish_load(&new_ja, true));
        assert_eq!(state.current, langid!("ja"));
        assert!(state.ready);
    }

    #[test]
    fn selecting_the_active_language_cancels_a_pending_load() {
        let mut state = LocaleState::new(langid!("en"));
        assert_eq!(state.picker_language(), &langid!("en"));
        let pending = state.select(langid!("ja")).expect("发起日语读取");
        assert_eq!(state.picker_language(), &langid!("ja"));
        assert_eq!(state.current, langid!("en"));

        assert!(state.select(langid!("en")).is_none());
        assert_eq!(state.picker_language(), &langid!("en"));
        assert!(!state.finish_load(&pending, true));
        assert_eq!(state.picker_language(), &langid!("en"));
        assert_eq!(state.current, langid!("en"));
        assert!(state.ready);
    }

    #[test]
    fn selecting_the_active_language_clears_the_latest_load_error() {
        let mut state = LocaleState::new(langid!("en"));
        let pending = state.select(langid!("ja")).expect("发起日语读取");
        assert_eq!(state.picker_language(), &langid!("ja"));
        assert!(state.finish_load(&pending, false));
        assert_eq!(state.error, Some("language-switch-failed"));
        assert_eq!(state.picker_language(), &langid!("en"));

        assert!(state.select(langid!("en")).is_none());
        assert_eq!(state.error, None);
    }

    #[test]
    #[should_panic(expected = "翻译 missing-key 失败")]
    fn translation_failures_panic_in_debug_builds() {
        // 测试构建开启调试断言：翻译问题必须在测试里暴露，不能被发布构建的回退掩盖。
        let _ = translation_failed("missing-key", &"unknown variable");
    }

    const DEFAULT_FTL: &str = "greeting = Hello\nfallback-only = Default fallback\n";
    static TEST_CATALOG: LocaleCatalog =
        LocaleCatalog { default_locale: langid!("en"), default_ftl: DEFAULT_FTL, sources: &[] };

    type SessionControl = Rc<Cell<Option<LocaleSession>>>;

    fn use_test_locale_session(initial: unic_langid::LanguageIdentifier) -> LocaleSession {
        let bundle = try_use_init_i18n_from_bundle(|| {
            build_bundle(&langid!("en"), DEFAULT_FTL, &initial, None)
        })
        .expect("初始化译文");
        use_context_provider(|| LocaleSession::new(bundle, initial))
    }

    #[component]
    fn LanguageHarness(initial: unic_langid::LanguageIdentifier, show_picker: bool) -> Element {
        let session = use_test_locale_session(initial);
        consume_context::<SessionControl>().set(Some(session));
        rsx! {
            CurrentLanguageText {}
            if show_picker {
                LanguagePicker { label: "语言".to_owned(), catalog: &TEST_CATALOG }
            }
        }
    }

    #[component]
    fn CurrentLanguageText() -> Element {
        let language = current_language();
        rsx! { "{language}" }
    }

    #[test]
    fn reading_language_in_a_render_subscribes_to_switches() {
        let control = SessionControl::default();
        let props = LanguageHarnessProps { initial: langid!("ja"), show_picker: false };
        let mut dom =
            VirtualDom::new_with_props(LanguageHarness, props).with_root_context(control.clone());
        assert!(dom.rebuild_to_vec().edits.iter().any(|edit| matches!(
            edit,
            Mutation::CreateTextNode { value, .. } if value == "ja"
        )));

        let mut session = control.get().expect("挂载语言会话");
        dom.in_scope(ScopeId::APP, || session.switch(&TEST_CATALOG, "en"));
        assert!(dom.render_immediate_to_vec().edits.iter().any(|edit| matches!(
            edit,
            Mutation::SetText { value, .. } if value == "en"
        )));
    }

    #[component]
    fn TranslationLabel(with_args: bool) -> Element {
        let text = if with_args {
            crate::t!("greeting", extra: "unused")
        } else {
            let key = "greeting".to_owned();
            crate::t!(&key)
        };
        rsx! { "{text}" }
    }

    fn translation_harness() -> Element {
        let initial = langid!("ja");
        let bundle = try_use_init_i18n_from_bundle(|| {
            build_bundle(
                &TEST_CATALOG.default_locale,
                DEFAULT_FTL,
                &initial,
                Some("greeting = \u{2068}こんにちは\u{2069}\n"),
            )
        })
        .expect("初始化译文");
        let session = use_context_provider(|| LocaleSession::new(bundle, initial));
        consume_context::<SessionControl>().set(Some(session));
        rsx! {
            TranslationLabel { with_args: false }
            TranslationLabel { with_args: true }
        }
    }

    #[test]
    fn translation_paths_follow_language_switches_and_report_missing_keys() {
        let control = SessionControl::default();
        let mut dom = VirtualDom::new(translation_harness).with_root_context(control.clone());
        let initial = dom.rebuild_to_vec();
        let initial_text: Vec<_> = initial
            .edits
            .iter()
            .filter_map(|edit| match edit {
                Mutation::CreateTextNode { value, .. } => Some(value.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(initial_text, ["こんにちは", "こんにちは"]);

        let mut session = control.get().expect("挂载语言会话");
        dom.in_scope(ScopeId::APP, || session.switch(&TEST_CATALOG, "en"));
        let updated = dom.render_immediate_to_vec();
        let updated_text: Vec<_> = updated
            .edits
            .iter()
            .filter_map(|edit| match edit {
                Mutation::SetText { value, .. } => Some(value.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(updated_text, ["Hello", "Hello"]);

        let missing =
            dom.in_scope(ScopeId::APP, || std::panic::catch_unwind(|| crate::t!("missing-key")));
        if cfg!(debug_assertions) {
            let error = missing.expect_err("调试构建报告缺键");
            assert!(
                error
                    .downcast_ref::<String>()
                    .is_some_and(|text| { text.starts_with("翻译 missing-key 失败") })
            );
        } else {
            assert_eq!(missing.expect("发布构建显示键名"), "missing-key");
        }
    }

    #[test]
    fn language_picker_renders_the_pending_choice_and_the_return_to_active_language() {
        let control = SessionControl::default();
        let props = LanguageHarnessProps { initial: langid!("en"), show_picker: true };
        let mut dom =
            VirtualDom::new_with_props(LanguageHarness, props).with_root_context(control.clone());
        let initial = dom.rebuild_to_vec();
        let picker = initial
            .edits
            .iter()
            .find_map(|edit| match edit {
                Mutation::NewEventListener { name, id } if name == "change" => Some(*id),
                _ => None,
            })
            .expect("挂载语言选择器");
        let picker_has_value = |edits: &[Mutation], expected: &str| {
            edits.iter().any(|edit| matches!(
                edit,
                Mutation::SetAttribute { id, name: "value", value: AttributeValue::Text(value), .. }
                    if *id == picker && value == expected
            ))
        };
        assert!(picker_has_value(&initial.edits, "en"));

        let mut session = control.get().expect("挂载语言会话");
        let pending =
            dom.in_runtime(|| session.state.write().select(langid!("ja"))).expect("日语尚在加载");
        assert!(picker_has_value(&dom.render_immediate_to_vec().edits, "ja"));
        assert_eq!(session.current(), langid!("en"));

        dom.in_scope(ScopeId::APP, || session.switch(&TEST_CATALOG, "en"));
        assert!(picker_has_value(&dom.render_immediate_to_vec().edits, "en"));
        assert!(!dom.in_runtime(|| session.state.write().finish_load(&pending, true)));
    }

    #[derive(Default)]
    struct LocaleOwnerControl {
        session: Cell<Option<LocaleSession>>,
        show_picker: Cell<Option<Signal<bool>>>,
        picker_scope: Cell<Option<ScopeId>>,
    }

    #[component]
    fn LocaleOwnerHarness() -> Element {
        let control = consume_context::<Rc<LocaleOwnerControl>>();
        let session = use_test_locale_session(langid!("ja"));
        let show_picker = use_signal(|| true);
        control.session.set(Some(session));
        control.show_picker.set(Some(show_picker));
        rsx! {
            if show_picker() {
                LocalePickerScope {}
            }
        }
    }

    #[component]
    fn LocalePickerScope() -> Element {
        consume_context::<Rc<LocaleOwnerControl>>()
            .picker_scope
            .set(Some(dioxus::dioxus_core::current_scope_id()));
        rsx! {}
    }

    #[test]
    fn language_loading_remains_owned_by_the_session_when_the_picker_unmounts() {
        let control = Rc::new(LocaleOwnerControl::default());
        let mut dom = VirtualDom::new(LocaleOwnerHarness).with_root_context(control.clone());
        dom.rebuild_in_place();
        let mut session = control.session.get().expect("挂载语言会话");
        let mut show_picker = control.show_picker.get().expect("挂载选择器显示信号");
        let scope = control.picker_scope.get().expect("挂载选择器子作用域");

        dom.in_scope(scope, || session.switch(&TEST_CATALOG, "en"));
        assert_eq!(session.state.peek().picker_language(), &langid!("en"));
        assert_eq!(session.current(), langid!("ja"));
        dom.in_runtime(|| show_picker.set(false));
        dom.render_immediate(&mut dioxus::dioxus_core::NoOpMutations);
        dom.process_events();

        assert!(dom.get_scope(scope).is_none());
        assert_eq!(session.current(), langid!("en"));
        assert!(session.state.peek().pending.is_none());
    }

    fn translate(
        bundle: &dioxus_i18n::fluent::FluentBundle<dioxus_i18n::fluent::FluentResource>,
        id: &str,
    ) -> String {
        let message = bundle.get_message(id).expect("message exists");
        let pattern = message.value().expect("message has value");
        let mut errors = Vec::new();
        let text = bundle.format_pattern(pattern, None, &mut errors).to_string();
        assert!(errors.is_empty(), "fluent errors: {errors:?}");
        text
    }

    #[test]
    fn target_overrides_and_falls_back_to_default() {
        let default = langid!("en");
        let target = langid!("zh-CN");
        let bundle = build_bundle(&default, DEFAULT_FTL, &target, Some("greeting = 你好\n"))
            .expect("valid bundle");
        assert_eq!(translate(&bundle, "greeting"), "你好");
        assert_eq!(translate(&bundle, "fallback-only"), "Default fallback");
    }

    #[test]
    fn default_language_ignores_extra_resource() {
        let default = langid!("en");
        let bundle = build_bundle(&default, DEFAULT_FTL, &default, Some("greeting = 你好\n"))
            .expect("valid bundle");
        assert_eq!(translate(&bundle, "greeting"), "Hello");
    }

    #[test]
    fn invalid_ftl_is_rejected() {
        let default = langid!("en");
        let target = langid!("ja");
        assert!(build_bundle(&default, DEFAULT_FTL, &target, Some("= broken\n")).is_err());
        assert!(build_bundle(&default, "= broken\n", &default, None).is_err());
    }
}
