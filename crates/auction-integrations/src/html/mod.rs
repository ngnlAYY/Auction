//! 常量 CSS selector 的一次解析：每个调用点各自缓存源码字面量的解析结果。字面量解析失败
//! 只可能是编码错误，首次失败记录一条 ERROR 诊断，之后同一调用点直接返回 `Err(())`；
//! 调用方把 `()` 映射为各自失败关闭的错误，不缓存任何外部正文。

/// 生成只解析一次的常量 selector，表达式类型为 `Result<&'static Selector, ()>`；
/// `kind`/`phase` 是解析失败时的诊断标签。
macro_rules! static_selector {
    ($kind:literal, $phase:literal, $css:literal $(,)?) => {{
        static SELECTOR: ::std::sync::LazyLock<::std::result::Result<::scraper::Selector, ()>> =
            ::std::sync::LazyLock::new(|| {
                ::scraper::Selector::parse($css).map_err($crate::safe_diagnostics::error_map(
                    $kind,
                    $phase,
                    || (),
                ))
            });
        match ::std::sync::LazyLock::force(&SELECTOR) {
            ::std::result::Result::Ok(selector) => ::std::result::Result::Ok(selector),
            ::std::result::Result::Err(()) => ::std::result::Result::Err(()),
        }
    }};
}

pub(crate) use static_selector;

#[cfg(test)]
mod tests {
    use scraper::Html;

    fn first_link() -> Result<&'static scraper::Selector, ()> {
        static_selector!("html_test", "build_link_selector", "a[href]")
    }

    /// 同一调用点只解析一次，返回同一个静态 selector；非法字面量失败关闭为 `Err(())`。
    #[test]
    fn literal_selector_is_parsed_once_per_call_site_and_invalid_fails_closed() {
        let first = first_link().expect("合法 selector");
        assert!(std::ptr::eq(first, first_link().expect("合法 selector")));
        let document = Html::parse_fragment("<p><a href=\"/x\">x</a><a>y</a></p>");
        assert_eq!(document.select(first).count(), 1);

        let invalid: Result<&scraper::Selector, ()> =
            static_selector!("html_test", "build_invalid_selector", "a[");
        assert!(invalid.is_err());
    }
}
