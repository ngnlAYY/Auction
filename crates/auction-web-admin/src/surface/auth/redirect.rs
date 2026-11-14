//! 登录跳转与站内 return 地址。

use dioxus::prelude::*;

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
        let search = window.location().search().unwrap_or_default();
        let raw = search.strip_prefix('?').unwrap_or_default();
        // 与生成端保持一致：拒绝登录自循环，以及浏览器会按网络地址解释的反斜线。
        for (key, value) in url::form_urlencoded::parse(raw.as_bytes()) {
            if key == "return"
                && value.starts_with('/')
                && !value.starts_with("//")
                && !value.contains('\\')
                && value.split(['?', '#']).next() != Some("/login")
                && !value.chars().any(char::is_control)
            {
                return value.into_owned();
            }
        }
    }
    "/".to_owned()
}
