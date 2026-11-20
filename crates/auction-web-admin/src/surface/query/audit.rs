//! 审计列表的 query 状态：九项筛选、页码与排序。
//! 表单草稿只在提交时写入 URL，无效历史值保留供操作员更正。

use auction_web_shared::{QueryState, use_query_state};

use super::super::audit::AuditFilterState;
#[cfg(test)]
use super::super::realtime::use_test_history;
use super::super::sort::SortDirection;
use super::primitives::{
    parse_page, parse_sort_column, parse_sort_direction, push_page, push_sort,
};
use crate::api::AdminAuditFilter;

const AUDIT_KEYS: &[&str] = &[
    "actor",
    "entity",
    "action",
    "request_id",
    "actor_type",
    "source",
    "result",
    "from",
    "to",
    "page",
    "sort",
    "dir",
];

/// 审计列表可排序列：发生时间、操作者、动作、实体、来源、结果。
const AUDIT_SORT_COLUMNS: &[&str] = &["at", "actor", "action", "entity", "source", "result"];

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AuditQuery {
    pub filter: AuditFilterState,
    pub page: i64,
    /// 排序列标识；空串表示保持服务端原始顺序（发生时间倒序）。合法值见 `AUDIT_SORT_COLUMNS`。
    pub sort: String,
    pub direction: SortDirection,
}

impl Default for AuditQuery {
    fn default() -> Self {
        Self {
            filter: AuditFilterState::default(),
            page: 1,
            sort: String::new(),
            direction: SortDirection::Asc,
        }
    }
}

impl AuditQuery {
    /// 提交筛选从第一页开始，并沿用当前排序。
    pub(crate) fn with_filter(self, filter: AdminAuditFilter) -> Self {
        Self { filter: filter.into(), page: 1, ..self }
    }

    /// 无效地址等待更正，不能按缺省筛选读取审计记录。
    pub(crate) fn page_request(&self) -> Option<(i64, AdminAuditFilter)> {
        Some((self.page, self.filter.request()?))
    }

    #[cfg(test)]
    pub(crate) fn restored_from_history(&self) -> Self {
        parse_audit_query(&audit_query_pairs(self))
    }
}

pub(crate) fn use_audit_query_state() -> QueryState<AuditQuery> {
    let location = use_query_state(AUDIT_KEYS, parse_audit_query, audit_query_pairs);
    #[cfg(test)]
    use_test_history(location.state, parse_audit_query, audit_query_pairs);
    location
}

fn parse_audit_query(pairs: &[(String, String)]) -> AuditQuery {
    AuditQuery {
        filter: AuditFilterState::restore(pairs),
        page: parse_page(pairs, "page"),
        sort: parse_sort_column(pairs, AUDIT_SORT_COLUMNS),
        direction: parse_sort_direction(pairs),
    }
}

fn audit_query_pairs(query: &AuditQuery) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    query.filter.append_pairs(&mut pairs);
    push_page(&mut pairs, "page", query.page);
    push_sort(&mut pairs, &query.sort, query.direction);
    pairs
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use auction_time::Timestamp;
    use auction_web_shared::query::{QueryLocation, use_query_router};
    use auction_web_shared::{QueryWriteMode, build_bundle, write_query_pairs};
    use dioxus::dioxus_core;
    use dioxus::dioxus_core::{DynamicNode, ScopeId, VNode};
    use dioxus::history::{History, MemoryHistory};
    use dioxus::prelude::component;
    use dioxus::prelude::dioxus_signals;
    use dioxus::prelude::{
        Element, ReadableExt, Router, Signal, VirtualDom, WritableExt, consume_context, rsx,
        use_hook, use_signal,
    };
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;

    use super::{AuditQuery, audit_query_pairs, parse_audit_query, use_audit_query_state};
    use crate::api::{AdminApiFailure, AdminAuditFilter};
    use crate::surface::ADMIN_LOCALE_CATALOG;
    use crate::surface::auth::AdminShell;
    use crate::surface::realtime::{get_parameter, text_attributes};
    use crate::surface::router::Route;

    #[test]
    fn submitted_and_restored_text_filters_share_one_url_identity() {
        let applied = AuditQuery::default().with_filter(AdminAuditFilter {
            actor: " alice ".to_owned(),
            entity: " auction:sample lot ".to_owned(),
            action: " auction.create ".to_owned(),
            request_id: "  ".to_owned(),
            ..AdminAuditFilter::default()
        });
        let values = [
            ("actor".to_owned(), "alice".to_owned()),
            ("entity".to_owned(), "auction:sample lot".to_owned()),
            ("action".to_owned(), "auction.create".to_owned()),
        ];
        assert_eq!(audit_query_pairs(&applied), values);
        assert_eq!(parse_audit_query(&values), applied);
        let invalid = [("actor".to_owned(), " invalid\0actor ".to_owned())];
        let restored = parse_audit_query(&invalid);
        assert_eq!(audit_query_pairs(&restored), invalid);
        assert!(restored.page_request().is_none());
    }

    #[test]
    fn legacy_local_audit_link_is_rewritten_with_an_absolute_time() {
        let query = parse_audit_query(&[("from".to_owned(), "2026-11-18T09:05".to_owned())]);
        let (_, filter) = query.page_request().unwrap();
        assert_eq!(filter.occurred_from, "2026-11-18T09:05:00Z");
        assert_eq!(audit_query_pairs(&query), [("from".to_owned(), filter.occurred_from)]);
    }

    #[test]
    fn offset_audit_link_round_trips_without_changing_the_instant() {
        let input = [("to".to_owned(), "2026-11-18T09:05:37.500123456+08:00".to_owned())];
        let query = parse_audit_query(&input);
        let serialized = audit_query_pairs(&query);
        assert_eq!(serialized, [("to".to_owned(), "2026-11-18T01:05:37.500123456Z".to_owned())]);
        assert_eq!(parse_audit_query(&serialized), query);
        assert_eq!(
            Timestamp::parse_from_rfc3339(&query.page_request().unwrap().1.occurred_to).unwrap(),
            Timestamp::parse_from_rfc3339(&input[0].1).unwrap()
        );
    }

    #[test]
    fn malformed_time_does_not_become_an_unbounded_filter() {
        let invalid = [("from".to_owned(), "2026-11-31T09:05:00+08:00".to_owned())];
        let query = parse_audit_query(&invalid);
        assert_eq!(audit_query_pairs(&query), invalid);
        assert!(query.page_request().is_none());
    }

    #[test]
    fn audit_recovery_keeps_repeated_keys_and_control_characters() {
        for (key, valid) in [
            ("actor", "alice"),
            ("entity", "auction:sample"),
            ("action", "auction.create"),
            ("request_id", "request-1"),
            ("actor_type", "admin"),
            ("source", "admin"),
            ("result", "success"),
            ("from", "2026-11-18T09:00:00Z"),
            ("to", "2026-11-18T10:00:00Z"),
        ] {
            let pairs = [(key.to_owned(), valid.to_owned())];
            assert!(parse_audit_query(&pairs).page_request().is_some());
            for pairs in [
                vec![(key.to_owned(), String::new()), (key.to_owned(), valid.to_owned())],
                vec![(key.to_owned(), format!("{valid}\n"))],
            ] {
                let query = parse_audit_query(&pairs);
                assert_eq!(audit_query_pairs(&query), pairs);
                assert!(query.page_request().is_none(), "{key}");
            }
        }
    }

    struct AuditBrowser {
        history: Rc<MemoryHistory>,
        route: Cell<Option<Signal<String>>>,
        requests: RefCell<Vec<String>>,
    }

    impl AuditBrowser {
        fn notify(&self) {
            self.route.get().unwrap().set(self.history.current_route());
        }
    }

    impl QueryLocation for AuditBrowser {
        fn current(&self) -> String {
            self.history.current_route()
        }

        fn write(&self, location: &str, mode: QueryWriteMode) -> bool {
            if self.current() != location {
                match mode {
                    QueryWriteMode::Push => self.history.push(location.to_owned()),
                    QueryWriteMode::Replace => self.history.replace(location.to_owned()),
                }
                self.notify();
            }
            true
        }
    }

    fn audit_root() -> Element {
        let browser = consume_context::<Rc<AuditBrowser>>();
        let route = use_signal(|| browser.current());
        browser.route.set(Some(route));
        use_query_router(move || route.read().clone());
        let catalog = &ADMIN_LOCALE_CATALOG;
        try_use_init_i18n_from_bundle(|| {
            build_bundle(
                &catalog.default_locale,
                catalog.default_ftl,
                &catalog.default_locale,
                None,
            )
        })
        .unwrap();
        use_hook(move || {
            AdminApiFailure::stub_audit_responses(move |url| {
                browser.requests.borrow_mut().push(url.to_owned());
                let page = get_parameter(url, "page").unwrap().parse::<i64>().unwrap();
                let page_size = get_parameter(url, "page_size").unwrap().parse::<i64>().unwrap();
                let body = serde_json::json!({
                    "code": "ok", "message": "已读取", "request_id": null,
                    "data": {
                        "records": [], "page": page, "page_size": page_size, "total": 200,
                        "has_more": page * page_size < 200, "next_cursor": null,
                        "next_cursor_token": ""
                    }
                });
                (200, body.to_string())
            });
        });
        rsx! { AuditPage {} }
    }

    #[component]
    fn AuditPage() -> Element {
        crate::surface::audit::page()
    }

    #[test]
    fn mounted_audit_query_changes_get_parameters_on_same_page_navigation() {
        let initial = "/audit?actor=alice&page=3&sort=actor&dir=desc&page_size=50";
        let browser = Rc::new(AuditBrowser {
            history: Rc::new(MemoryHistory::with_initial_path(initial)),
            route: Cell::new(None),
            requests: RefCell::new(Vec::new()),
        });
        let mut dom = VirtualDom::new(audit_root)
            .with_root_context(browser.clone())
            .with_root_context(browser.clone() as Rc<dyn QueryLocation>)
            .with_root_context(browser.history.clone() as Rc<dyn History>);
        let initial_edits = dom.rebuild_to_vec().edits;
        let mut step = |action: &dyn Fn(), requested: &[&str]| {
            dom.in_runtime(action);
            let mut edits = Vec::new();
            for _ in 0..4 {
                edits.extend(dom.render_immediate_to_vec().edits);
            }
            assert_eq!(browser.requests.take(), requested);
            edits
        };
        let filtered = "/api/v1/admin/audit?page=3&page_size=50&actor=alice";
        let defaults = "/api/v1/admin/audit?page=1&page_size=10";
        let first_get_edits = step(&|| {}, &[filtered]);
        assert!(text_attributes(&initial_edits, "value").any(|(_, value)| value == "alice"));
        assert!(
            text_attributes(&first_get_edits, "aria-sort").any(|(_, value)| value == "descending")
        );
        let edits = step(
            &|| {
                browser.write("/audit", QueryWriteMode::Push);
            },
            &[defaults],
        );
        assert!(text_attributes(&edits, "value").any(|(_, value)| value.is_empty()));
        assert!(!text_attributes(&edits, "aria-sort").any(|(_, value)| value == "descending"));
        step(
            &|| {
                browser.history.go_back();
                browser.notify();
            },
            &[filtered],
        );
        step(
            &|| {
                browser.history.go_forward();
                browser.notify();
            },
            &[defaults],
        );
        step(
            &|| {
                browser.write("/audit?actor=bob&actor=carol", QueryWriteMode::Replace);
            },
            &[],
        );
        let edits = step(
            &|| {
                browser.write("/audit?actor=dave&page=2", QueryWriteMode::Replace);
            },
            &["/api/v1/admin/audit?page=2&page_size=10&actor=dave"],
        );
        assert!(text_attributes(&edits, "value").any(|(_, value)| value == "dave"));
    }

    #[test]
    fn mounted_audit_navigation_before_page_size_write_sends_unfiltered_get() {
        let browser = Rc::new(AuditBrowser {
            history: Rc::new(MemoryHistory::with_initial_path("/audit?actor=alice")),
            route: Cell::new(None),
            requests: RefCell::new(Vec::new()),
        });
        let mut dom = VirtualDom::new(audit_root)
            .with_root_context(browser.clone())
            .with_root_context(browser.clone() as Rc<dyn QueryLocation>)
            .with_root_context(browser.history.clone() as Rc<dyn History>);
        let initial = dom.rebuild_to_vec();
        assert!(text_attributes(&initial.edits, "value").any(|(_, value)| value == "alice"));
        for _ in 0..4 {
            dom.render_immediate_to_vec();
        }
        assert_eq!(
            browser.requests.take(),
            ["/api/v1/admin/audit?page=1&page_size=10&actor=alice"]
        );

        dom.in_scope(dioxus_core::ScopeId::APP, || {
            assert!(browser.write("/audit", QueryWriteMode::Push));
            assert!(write_query_pairs(
                &["page_size"],
                &[("page_size".to_owned(), "20".to_owned())],
                QueryWriteMode::Replace,
            ));
        });
        let mut edits = Vec::new();
        for _ in 0..4 {
            edits.extend(dom.render_immediate_to_vec().edits);
        }
        assert_eq!(browser.current(), "/audit?page_size=20");
        assert_eq!(browser.requests.take(), ["/api/v1/admin/audit?page=1&page_size=20"]);
        assert!(text_attributes(&edits, "value").any(|(_, value)| value.is_empty()));
    }

    fn router_root() -> Element {
        let catalog = &ADMIN_LOCALE_CATALOG;
        try_use_init_i18n_from_bundle(|| {
            build_bundle(
                &catalog.default_locale,
                catalog.default_ftl,
                &catalog.default_locale,
                None,
            )
        })
        .unwrap();
        rsx! { Router::<Route> {} }
    }

    fn admin_shell_scope(dom: &VirtualDom, node: &VNode) -> Option<ScopeId> {
        node.dynamic_nodes.iter().enumerate().find_map(|(index, child)| match child {
            DynamicNode::Component(component) => {
                let scope = component.mounted_scope(index, node, dom)?;
                if component.name == std::any::type_name_of_val(&AdminShell) {
                    Some(scope.id())
                } else {
                    admin_shell_scope(dom, scope.root_node())
                }
            }
            DynamicNode::Fragment(nodes) => {
                nodes.iter().find_map(|child| admin_shell_scope(dom, child))
            }
            _ => None,
        })
    }

    #[test]
    fn mounted_admin_shell_provides_route_query_to_audit_subscriber() {
        let history = Rc::new(MemoryHistory::with_initial_path("/audit"));
        let mut dom =
            VirtualDom::new(router_root).with_root_context(history.clone() as Rc<dyn History>);
        dom.rebuild_to_vec();
        let root = dom.get_scope(ScopeId::APP).unwrap().root_node();
        let shell = admin_shell_scope(&dom, root).expect("生产路由挂载了管理端外壳");

        // Router 首次挂载会规范地址；挂载后为查询订阅者准备完整地址。
        history.replace("/audit?actor=alice".to_owned());
        // 会话替身在认证模块内可见。会话确认前单独在外壳作用域运行真实查询订阅者。
        dom.in_scope(shell, || {
            let query = use_audit_query_state();
            let (page, filter) = query.state.read().page_request().unwrap();
            assert_eq!(page, 1);
            assert_eq!(filter.actor, "alice");
        });
    }
}
