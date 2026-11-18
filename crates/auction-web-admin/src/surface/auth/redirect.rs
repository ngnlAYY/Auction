//! 登录跳转与站内 return 地址。

use dioxus::prelude::*;
#[cfg(any(target_arch = "wasm32", test))]
use url::{Position, Url};

#[cfg(any(target_arch = "wasm32", test))]
use crate::surface::router::Route;

/// 未认证访问受保护页面时，按旧版保存站内 return 地址并替换到登录页。
#[component]
pub(crate) fn AdminLoginRedirect() -> Element {
    let return_target = current_admin_location();
    let encoded_return =
        url::form_urlencoded::byte_serialize(return_target.as_bytes()).collect::<String>();
    let target = format!("/login?return={encoded_return}");
    use_effect(move || {
        let _ = navigator().replace(target.clone());
    });
    rsx! {}
}

/// 已认证用户访问登录路径时，按旧版 return 参数回到原始站内地址。
#[component]
pub(crate) fn AdminReturnRedirect() -> Element {
    let target = admin_return_path();
    use_effect(move || {
        let _ = navigator().replace(target.clone());
    });
    rsx! {}
}

#[cfg(target_arch = "wasm32")]
fn current_admin_location() -> String {
    let Some(window) = web_sys::window() else {
        return "/".to_owned();
    };
    let location = window.location();
    let pathname = location.pathname().unwrap_or_else(|_| "/".to_owned());
    let search = location.search().unwrap_or_default();
    let hash = location.hash().unwrap_or_default();
    if pathname.starts_with('/') && pathname != "/login" {
        format!("{pathname}{search}{hash}")
    } else {
        "/".to_owned()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn current_admin_location() -> String {
    "/".to_owned()
}

fn admin_return_path() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return "/".to_owned();
        };
        let location = window.location();
        let Some(origin) = location.origin().ok().and_then(|origin| Url::parse(&origin).ok())
        else {
            return "/".to_owned();
        };
        return_path_for_query(&location.search().unwrap_or_default(), &origin)
    }
    #[cfg(not(target_arch = "wasm32"))]
    "/".to_owned()
}

#[cfg(any(target_arch = "wasm32", test))]
fn return_path_for_query(search: &str, origin: &Url) -> String {
    let raw = search.strip_prefix('?').unwrap_or_default();
    let Some((_, value)) =
        url::form_urlencoded::parse(raw.as_bytes()).find(|(key, _)| key == "return")
    else {
        return "/".to_owned();
    };
    if !value.starts_with('/')
        || value.starts_with("//")
        || value.contains('\\')
        || value.chars().any(char::is_control)
    {
        return "/".to_owned();
    }
    let Ok(normalized) = origin.join(&value) else { return "/".to_owned() };
    let path = normalized.path();
    // 浏览器会消解点段；路径中另拒绝重复斜线与编码分隔符，避免路由和 URL 对段边界解释不同。
    if normalized.origin() != origin.origin()
        || &normalized[Position::BeforePath..] != value.as_ref()
        || path.contains("//")
        || path
            .as_bytes()
            .windows(3)
            .any(|part| part.eq_ignore_ascii_case(b"%2f") || part.eq_ignore_ascii_case(b"%5c"))
        || !path.parse::<Route>().is_ok_and(|route| !matches!(route, Route::Login {}))
    {
        return "/".to_owned();
    }
    value.into_owned()
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::return_path_for_query;

    #[test]
    fn admin_login_return_accepts_local_query_and_fragment() {
        let origin = Url::parse("https://admin.example.test").unwrap();
        assert_eq!(
            return_path_for_query("?return=%2Faudit%3Fpage%3D2%26kind%3Dlogin%23audit", &origin),
            "/audit?page=2&kind=login#audit"
        );
        assert_eq!(
            return_path_for_query("?return=%2Faudit%3Fquery%3D%252Fitem%23list", &origin),
            "/audit?query=%2Fitem#list"
        );
    }

    #[test]
    fn admin_login_return_rejects_login_variants_and_external_targets() {
        let origin = Url::parse("https://admin.example.test").unwrap();
        for query in [
            "?return=%2Flogin%2F",
            "?return=%2F%256Cogin",
            "?return=%2Flogin%3Fpage%3D2",
            "?return=%2F%2Fexample.test",
            "?return=%2F%5Cexample.test",
            "?return=%2Faudit%2F..%2Flogin%2F",
            "?return=%2Faudit%2F%252e%252e%2Flogin%2F",
            "?return=%2Faudit%5C..%5Clogin%5C",
            "?return=%2Faudit%252F..%252Flogin",
            "?return=%2Faudit%2F%2Flogin",
            "?return=https%3A%2F%2Fexample.test",
            "?return=%2F%0Aexample.test",
        ] {
            assert_eq!(return_path_for_query(query, &origin), "/", "{query}");
        }
    }
}
