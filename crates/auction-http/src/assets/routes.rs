//! 后端批准提供 SPA 入口的浏览器路由清单。
//!
//! 这里与两个 Dioxus `Route` 枚举共同冻结浏览器路径。新增或删除页面时必须同步修改对应枚举、
//! 本清单和 `docs/contracts/routes.md`；未知路径不能自动获得 `index.html`。

const PUBLIC_EXACT_ROUTES: &[&str] = &[
    "/",
    "/account",
    "/auction",
    "/auctions",
    "/bidlog",
    "/blacklist",
    "/compensation",
    "/compensation-pool",
    "/forum",
    "/history",
    "/login",
    "/me",
    "/rules",
    "/status",
];

const ADMIN_EXACT_ROUTES: &[&str] = &[
    "/",
    "/audit",
    "/auction",
    "/auctions",
    "/config",
    "/cookies",
    "/credentials",
    "/dashboard",
    "/deliveries",
    "/delivery",
    "/login",
    "/mail",
    "/mails",
    "/outbox",
    "/renames",
    "/rules",
    "/rules-content",
    "/settlement",
    "/status",
    "/users",
];

#[derive(Clone, Copy)]
pub(super) enum Surface {
    Public,
    Admin,
}

pub(super) fn browser_route(surface: Surface, path: &str) -> bool {
    match surface {
        Surface::Public => {
            PUBLIC_EXACT_ROUTES.contains(&path)
                || path.strip_prefix("/lots/").is_some_and(valid_parameter)
        }
        Surface::Admin => {
            ADMIN_EXACT_ROUTES.contains(&path)
                || path
                    .strip_prefix("/auctions/")
                    .and_then(|value| value.strip_suffix("/lots"))
                    .is_some_and(valid_parameter)
        }
    }
}

fn valid_parameter(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value, "." | "..")
        && value
            .bytes()
            .all(|value| !value.is_ascii_control() && !matches!(value, b'/' | b'\\' | b'%'))
}
