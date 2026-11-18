//! 后端批准提供 SPA 入口的浏览器路由清单。
//! 这里与两个 Dioxus `Route` 枚举、Solid 实现的路由表（`web/*/src/surface/router/route.ts`）
//! 共同冻结浏览器路径。新增或删除页面时必须同步修改对应枚举或路由表、本清单和
//! `docs/contracts/routes.md`；未知路径不能自动获得 `index.html`。
//! 只有一套实现提供的页面（如 Public 的 `/settings`）在另一套实现里落到未匹配路由，回到首页。

const PUBLIC_EXACT_ROUTES: &[&str] = &[
    "/",
    "/about",
    "/account",
    "/auction",
    "/auctions",
    "/bidlog",
    "/blacklist",
    "/compensation",
    "/compensation-pool",
    "/forum",
    "/fulfillment",
    "/history",
    "/login",
    "/me",
    "/rules",
    "/settings",
    "/status",
];

const ADMIN_EXACT_ROUTES: &[&str] = &[
    "/",
    "/about-content",
    "/alerts",
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
    "/player-support",
    "/renames",
    "/rules",
    "/rules-content",
    "/settlement",
    "/settlement-returns",
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
                || path.strip_prefix("/deliveries/").is_some_and(valid_parameter)
                || path.strip_prefix("/settlement-returns/").is_some_and(valid_parameter)
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

#[cfg(test)]
mod tests {
    use super::{Surface, browser_route};

    /// `/settings` 只有 Public 的 Solid 实现提供页面；Admin 没有这个入口，带尾随斜杠的写法也不给入口。
    #[test]
    fn settings_page_is_a_public_only_browser_route() {
        assert!(browser_route(Surface::Public, "/settings"));
        assert!(!browser_route(Surface::Admin, "/settings"));
        assert!(!browser_route(Surface::Public, "/settings/"));
    }

    /// 结算退件列表与详情只给 Admin SPA 入口；参数段为空、含路径分隔或穿越时不给入口。
    #[test]
    fn settlement_return_pages_are_admin_only_browser_routes() {
        let case = "/settlement-returns/0f8fad5b-d9cb-469f-a165-70867728950e";
        assert!(browser_route(Surface::Admin, "/settlement-returns"));
        assert!(browser_route(Surface::Admin, case));
        assert!(!browser_route(Surface::Public, "/settlement-returns"));
        assert!(!browser_route(Surface::Public, case));
        for invalid in [
            "/settlement-returns/",
            "/settlement-returns/..",
            "/settlement-returns/a/b",
            "/settlement-returns/a%2Fb",
        ] {
            assert!(!browser_route(Surface::Admin, invalid), "{invalid}");
        }
    }
}
