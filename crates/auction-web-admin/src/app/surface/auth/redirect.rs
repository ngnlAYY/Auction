use super::*;

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

pub(crate) fn admin_is_login_path() -> bool {
    current_admin_path().as_deref() == Some("/login")
}

#[cfg(target_arch = "wasm32")]
fn current_admin_path() -> Option<String> {
    web_sys::window().and_then(|window| window.location().pathname().ok())
}

#[cfg(not(target_arch = "wasm32"))]
fn current_admin_path() -> Option<String> {
    None
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
        for (key, value) in url::form_urlencoded::parse(raw.as_bytes()) {
            if key == "return"
                && value.starts_with('/')
                && !value.starts_with("//")
                && !value.chars().any(char::is_control)
            {
                return value.into_owned();
            }
        }
    }
    "/".to_owned()
}
