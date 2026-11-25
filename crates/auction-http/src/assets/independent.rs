//! 独立入口分类：哪些批准的浏览器路由永远由新版 Solid 页面提供。

use super::routes::{Surface, browser_route};

/// 页面请求取哪个入口 build。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EntryClass {
    /// 入口指针所指 build（可能是旧壳）。
    Default,
    /// 新版 Solid 入口；旧壳没有这些页面，也不会获得它们。
    Independent,
}

/// 凡 `/consignment` 前缀（Public 与 Admin）的批准路由一律是独立入口：旧壳没有任何寄售页面，
/// 前缀规则比逐路径白名单更不易漏登新页。路径必须先是批准的浏览器路由，未批准的路径仍走 404。
pub(super) fn entry_class(surface: Surface, path: &str) -> EntryClass {
    let under_consignment = path == "/consignment" || path.starts_with("/consignment/");
    if under_consignment && browser_route(surface, path) {
        EntryClass::Independent
    } else {
        EntryClass::Default
    }
}

#[cfg(test)]
mod tests {
    use super::{EntryClass, entry_class};
    use crate::assets::routes::Surface;

    #[test]
    fn consignment_prefix_routes_are_independent_and_the_rest_are_default() {
        for surface in [Surface::Public, Surface::Admin] {
            assert_eq!(entry_class(surface, "/consignment"), EntryClass::Independent);
            assert_eq!(entry_class(surface, "/"), EntryClass::Default);
            assert_eq!(entry_class(surface, "/status"), EntryClass::Default);
            // 前缀相似但不在批准集合内的路径不获得独立入口。
            assert_eq!(entry_class(surface, "/consignments"), EntryClass::Default);
            assert_eq!(entry_class(surface, "/consignment/unknown"), EntryClass::Default);
        }
        assert_eq!(
            entry_class(Surface::Public, "/consignment/me/0f8fad5b-d9cb-469f-a165-70867728950e"),
            EntryClass::Independent
        );
        assert_eq!(
            entry_class(
                Surface::Admin,
                "/consignment/deposits/0f8fad5b-d9cb-469f-a165-70867728950e"
            ),
            EntryClass::Independent
        );
    }
}
