//! 后端批准提供 SPA 入口的浏览器路由清单。
//! 这里与两个 Dioxus `Route` 枚举、Solid 实现的路由表（`web/*/src/surface/router/route.ts`）
//! 共同冻结浏览器路径。新增或删除页面时必须同步修改对应枚举或路由表、本清单和
//! `docs/contracts/routes.md`；未知路径不能自动获得 `index.html`。
//! 只有 Solid 提供的页面在 Rust 版触发各站的未匹配处理。

use crate::query::parse_canonical_uuid;

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
    "/consignment",
    "/consignment/me",
    "/consignment/new",
    "/consignment/orders",
    "/consignment/rights",
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
    "/consignment",
    "/consignment/orders",
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
    if let Some(listing_id) = path.strip_prefix("/consignment/listings/")
        && parse_canonical_uuid(listing_id).is_some()
    {
        return true;
    }
    match surface {
        Surface::Public => {
            PUBLIC_EXACT_ROUTES.contains(&path)
                || path.strip_prefix("/lots/").is_some_and(valid_parameter)
                || path
                    .strip_suffix("/deposit-instructions")
                    .and_then(|prefix| prefix.strip_prefix("/consignment/me/"))
                    .is_some_and(|listing_id| parse_canonical_uuid(listing_id).is_some())
                || path
                    .strip_suffix("/relist")
                    .and_then(|prefix| prefix.strip_prefix("/consignment/me/"))
                    .is_some_and(|listing_id| parse_canonical_uuid(listing_id).is_some())
                || path
                    .strip_suffix("/purchase")
                    .and_then(|prefix| prefix.strip_prefix("/consignment/listings/"))
                    .is_some_and(|listing_id| parse_canonical_uuid(listing_id).is_some())
                || path
                    .strip_prefix("/consignment/me/")
                    .is_some_and(|listing_id| parse_canonical_uuid(listing_id).is_some())
                || path
                    .strip_prefix("/consignment/orders/")
                    .is_some_and(|contract_id| parse_canonical_uuid(contract_id).is_some())
                || path
                    .strip_prefix("/consignment/loss/")
                    .is_some_and(|position_id| parse_canonical_uuid(position_id).is_some())
        }
        Surface::Admin => {
            ADMIN_EXACT_ROUTES.contains(&path)
                || path
                    .strip_prefix("/consignment/deposits/")
                    .is_some_and(|quarantine_id| parse_canonical_uuid(quarantine_id).is_some())
                || path
                    .strip_prefix("/consignment/orders/")
                    .is_some_and(|contract_id| parse_canonical_uuid(contract_id).is_some())
                || path
                    .strip_suffix("/loss-case")
                    .and_then(|prefix| prefix.strip_prefix("/consignment/positions/"))
                    .is_some_and(|position_id| parse_canonical_uuid(position_id).is_some())
                || path.strip_circumfix("/auctions/", "/lots").is_some_and(valid_parameter)
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

    /// 第 6、7 期的寄售页面只给对应站点入口：购买、订单、权利、重列与损失回执在 Public，
    /// 订单与损失案件在 Admin；参数段必须是规范 UUID。
    #[test]
    fn consignment_order_pages_are_surface_scoped_browser_routes() {
        let id = "0f8fad5b-d9cb-469f-a165-70867728950e";
        for path in [
            "/consignment/orders".to_owned(),
            "/consignment/rights".to_owned(),
            format!("/consignment/orders/{id}"),
            format!("/consignment/listings/{id}/purchase"),
            format!("/consignment/me/{id}/relist"),
            format!("/consignment/loss/{id}"),
        ] {
            assert!(browser_route(Surface::Public, &path), "{path}");
        }
        for path in [
            "/consignment/orders".to_owned(),
            format!("/consignment/orders/{id}"),
            format!("/consignment/positions/{id}/loss-case"),
        ] {
            assert!(browser_route(Surface::Admin, &path), "{path}");
        }
        assert!(!browser_route(Surface::Admin, "/consignment/rights"));
        assert!(!browser_route(Surface::Public, &format!("/consignment/positions/{id}/loss-case")));
        for invalid in
            ["/consignment/orders/x", "/consignment/listings/x/purchase", "/consignment/loss/x"]
        {
            assert!(!browser_route(Surface::Public, invalid), "{invalid}");
        }
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
