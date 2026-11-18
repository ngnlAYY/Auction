//! 用户列表的 query 状态：用户名、状态筛选、页码与排序。名称输入框是提交前草稿，不入 URL。

use auction_contracts::AdminUserPaginationResponse;
#[cfg(target_arch = "wasm32")]
use auction_web_shared::write_query_pairs;
use auction_web_shared::{QueryState, QueryWriteMode, use_query_state};
use dioxus::prelude::{Signal, WritableExt};

use super::super::sort::SortDirection;
use super::primitives::{
    pair_value, parse_page, parse_sort_column, parse_sort_direction, parse_text, push_page,
    push_sort, push_text,
};
use crate::api::AdminUserStatusFilter;
use crate::surface::users::fields::USER_NAME_BYTES;

const USERS_KEYS: &[&str] = &["name", "status", "page", "sort", "dir"];

/// 用户列表可排序列：用户名、状态、拉黑次数、创建时间。
const USERS_SORT_COLUMNS: &[&str] = &["username", "status", "blacklist_count", "created_at"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UsersQuery {
    pub name: String,
    pub status: AdminUserStatusFilter,
    pub page: i64,
    /// 排序列标识；空串表示保持服务端原始顺序。合法值见 `USERS_SORT_COLUMNS`。
    pub sort: String,
    pub direction: SortDirection,
}

impl Default for UsersQuery {
    fn default() -> Self {
        Self {
            name: String::new(),
            status: AdminUserStatusFilter::All,
            page: 1,
            sort: String::new(),
            direction: SortDirection::Asc,
        }
    }
}

impl UsersQuery {
    /// 地址栏写入成功后才发布新条件；浏览历史恢复与首次挂载仍共用同一解析器。
    pub(in crate::surface) fn apply(self, state: Signal<Self>) {
        self.write(state, QueryWriteMode::Push);
    }

    /// 只有与当前请求同页的权威响应才有资格纠正页码。
    pub(in crate::surface) fn page_correction(
        &self,
        response: &AdminUserPaginationResponse,
    ) -> Option<i64> {
        if response.page != self.page {
            return None;
        }
        let last_page = 1 + (response.total.max(1) - 1) / response.page_size.max(1);
        (self.page > last_page).then_some(last_page)
    }

    /// 无需纠页或成功写回时返回 true，写入失败时保留原查询。
    pub(in crate::surface) fn correct_page(
        self,
        state: Signal<Self>,
        response: &AdminUserPaginationResponse,
    ) -> bool {
        let Some(page) = self.page_correction(response) else {
            return true;
        };
        // 后退落在超出结果范围的页时，原地修正以保留之后的历史项。
        Self { page, ..self }.write(state, QueryWriteMode::Replace)
    }

    fn write(self, mut state: Signal<Self>, mode: QueryWriteMode) -> bool {
        // 请求仍须对应地址栏的查询，只有历史写入成功才公布新页码。
        if !self.write_address(mode) {
            return false;
        }
        state.set(self);
        true
    }

    fn write_address(&self, mode: QueryWriteMode) -> bool {
        #[cfg(test)]
        if let Some(history) = tests::History::from_context() {
            return history.borrow_mut().write(serialize_query(self), mode);
        }
        #[cfg(target_arch = "wasm32")]
        {
            write_query_pairs(USERS_KEYS, &serialize_query(self), mode)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (self, mode);
            true
        }
    }
}

/// 状态筛选序列化：`All` 是默认值，不写地址栏。
fn users_status_value(status: AdminUserStatusFilter) -> Option<&'static str> {
    match status {
        AdminUserStatusFilter::All => None,
        AdminUserStatusFilter::Active => Some("active"),
        AdminUserStatusFilter::Blacklisted => Some("blacklisted"),
    }
}

pub(crate) fn use_users_query_state() -> QueryState<UsersQuery> {
    let query = use_query_state(USERS_KEYS, parse_query, serialize_query);
    #[cfg(test)]
    if let Some(history) = tests::History::from_context() {
        history.borrow_mut().query = Some(query.state);
    }
    query
}

fn parse_query(pairs: &[(String, String)]) -> UsersQuery {
    UsersQuery {
        name: parse_text(pairs, "name", USER_NAME_BYTES),
        status: match pair_value(pairs, "status") {
            Some("active") => AdminUserStatusFilter::Active,
            Some("blacklisted") => AdminUserStatusFilter::Blacklisted,
            _ => AdminUserStatusFilter::All,
        },
        page: parse_page(pairs, "page"),
        sort: parse_sort_column(pairs, USERS_SORT_COLUMNS),
        direction: parse_sort_direction(pairs),
    }
}

fn serialize_query(query: &UsersQuery) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    push_text(&mut pairs, "name", &query.name);
    if let Some(status) = users_status_value(query.status) {
        pairs.push(("status".to_owned(), status.to_owned()));
    }
    push_page(&mut pairs, "page", query.page);
    push_sort(&mut pairs, &query.sort, query.direction);
    pairs
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, cell::RefCell, rc::Rc};

    use auction_contracts::AdminUserPaginationResponse;
    use auction_web_shared::i18n::{DEFAULT_ADMIN_LOCALE, build_bundle};
    use auction_web_shared::{QueryWriteMode, t};
    use dioxus::dioxus_core::{AttributeValue, Mutation, Mutations};
    use dioxus::prelude::*;
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;

    use super::{UsersQuery, parse_query, serialize_query};
    use crate::api::{AdminUserPage, AdminUserStatusFilter};
    use crate::surface::ADMIN_LOCALE_CATALOG;
    use crate::surface::users::{UsersPageFixture, page};

    pub(super) struct History {
        entries: Vec<Vec<(String, String)>>,
        index: usize,
        pub(super) query: Option<Signal<UsersQuery>>,
        fail_replace: bool,
        replace_attempts: usize,
    }

    impl History {
        fn new(initial: &UsersQuery) -> Self {
            Self {
                entries: vec![serialize_query(initial)],
                index: 0,
                query: None,
                fail_replace: false,
                replace_attempts: 0,
            }
        }

        pub(super) fn from_context() -> Option<Rc<RefCell<Self>>> {
            try_consume_context()
        }

        pub(super) fn write(&mut self, pairs: Vec<(String, String)>, mode: QueryWriteMode) -> bool {
            match mode {
                QueryWriteMode::Push => {
                    self.index += 1;
                    self.entries.truncate(self.index);
                    self.entries.push(pairs);
                }
                QueryWriteMode::Replace => {
                    self.replace_attempts += 1;
                    if self.fail_replace {
                        return false;
                    }
                    self.entries[self.index] = pairs;
                }
            }
            true
        }

        fn current_query(&self) -> UsersQuery {
            parse_query(&self.entries[self.index])
        }

        fn navigate(&mut self, index: usize) {
            self.index = index;
            let mut query = self.query.unwrap();
            query.set(self.current_query());
        }
    }

    #[test]
    fn history_restores_each_applied_name_and_status() {
        let first = UsersQuery {
            name: "玩家-A".into(),
            status: AdminUserStatusFilter::Active,
            ..UsersQuery::default()
        };
        let second = UsersQuery {
            name: "玩家-B".into(),
            status: AdminUserStatusFilter::Blacklisted,
            page: 2,
            ..UsersQuery::default()
        };
        let entries = [&first, &second].map(serialize_query);
        for index in [0, 1, 0, 1] {
            assert_eq!(&parse_query(&entries[index]), [&first, &second][index]);
        }
        assert_eq!(
            parse_query(&[("name".into(), "界".repeat(86)), ("status".into(), "invalid".into())]),
            UsersQuery::default()
        );
    }

    #[test]
    fn correcting_a_shrunken_page_keeps_the_forward_search() {
        let mut dom = VirtualDom::new(|| rsx! { div {} });
        dom.rebuild_in_place();
        dom.in_scope(ScopeId::APP, || {
            let a = UsersQuery { name: "A".into(), ..UsersQuery::default() };
            let b = UsersQuery { name: "B".into(), ..UsersQuery::default() };
            let history = Rc::new(RefCell::new(History::new(&a)));
            provide_context(history.clone());
            let mut state = Signal::new(a.clone());

            UsersQuery { page: 2, ..a.clone() }.apply(state);
            assert_eq!(history.borrow().index, 1);
            state().correct_page(
                state,
                &AdminUserPaginationResponse { page: 2, page_size: 10, total: 11 },
            );
            assert_eq!(state().page, 2);
            b.clone().apply(state);
            assert_eq!(history.borrow().index, 2);

            history.borrow_mut().index -= 1;
            state.set(parse_query(&history.borrow().entries[1]));
            assert_eq!(state().page, 2);
            state().correct_page(
                state,
                &AdminUserPaginationResponse { page: 1, page_size: 10, total: 8 },
            );
            assert_eq!(state().page, 2, "其他页的回执不能纠偏");
            state().correct_page(
                state,
                &AdminUserPaginationResponse { page: 2, page_size: 10, total: 8 },
            );
            assert_eq!(state(), a);
            assert_eq!(history.borrow().index, 1);
            assert_eq!(parse_query(&history.borrow().entries[1]), a);

            history.borrow_mut().index += 1;
            let history = history.borrow();
            state.set(parse_query(&history.entries[history.index]));
            assert_eq!(state(), b);
        });
    }

    fn users_page() -> Element {
        try_use_init_i18n_from_bundle(|| {
            build_bundle(
                &DEFAULT_ADMIN_LOCALE,
                ADMIN_LOCALE_CATALOG.default_ftl,
                &DEFAULT_ADMIN_LOCALE,
                None,
            )
        })
        .unwrap();
        page()
    }

    fn settle_users_page(dom: &mut VirtualDom) -> Mutations {
        let mut mutations = Mutations::default();
        for _ in 0..8 {
            dom.render_immediate(&mut mutations);
        }
        mutations
    }

    #[test]
    fn users_page_retries_failed_correction_and_keeps_the_forward_search() {
        let history = Rc::new(RefCell::new(History::new(&UsersQuery::default())));
        let requests = Rc::new(RefCell::new(Vec::new()));
        let a_total = Rc::new(Cell::new(11));
        let fixture = Rc::new(UsersPageFixture {
            fetch: Box::new({
                let requests = requests.clone();
                let a_total = a_total.clone();
                move |name, page, page_size| {
                    let total = if name == "A" { a_total.get() } else { 0 };
                    requests.borrow_mut().push((name, page));
                    AdminUserPage {
                        users: Vec::new(),
                        pagination: AdminUserPaginationResponse { page, page_size, total },
                    }
                }
            }),
            retry_correction: Cell::new(None),
        });
        let mut dom = VirtualDom::new(users_page)
            .with_root_context(history.clone())
            .with_root_context(fixture.clone());
        dom.rebuild(&mut Mutations::default());
        settle_users_page(&mut dom);
        let state = history.borrow().query.unwrap();

        let a = UsersQuery { name: "A".into(), page: 2, ..UsersQuery::default() };
        let b = UsersQuery { name: "B".into(), ..UsersQuery::default() };
        dom.in_scope(ScopeId::APP, || a.clone().apply(state));
        settle_users_page(&mut dom);
        assert_eq!(state(), a);
        assert_eq!(history.borrow().replace_attempts, 0);

        dom.in_scope(ScopeId::APP, || b.clone().apply(state));
        settle_users_page(&mut dom);
        assert_eq!(history.borrow().current_query(), b);

        a_total.set(8);
        history.borrow_mut().fail_replace = true;
        dom.in_runtime(|| history.borrow_mut().navigate(1));
        let error = settle_users_page(&mut dom);
        assert_eq!(state(), a);
        assert_eq!(history.borrow().current_query(), a);
        assert_eq!(history.borrow().replace_attempts, 1);
        assert!(
            error.edits.iter().any(|edit| matches!(edit,
                Mutation::SetAttribute { name: "role", value: AttributeValue::Text(value), .. }
                if value == "alert"
            )),
            "地址写入失败后，用户页应呈现错误"
        );
        let labels = dom.in_scope(ScopeId::APP, || {
            [t!("admin-users-page-correction-failed"), t!("admin-retry")]
        });
        for label in labels {
            assert!(error.edits.iter().any(|edit| matches!(edit,
                Mutation::CreateTextNode { value, .. } | Mutation::SetText { value, .. }
                if value == &label
            )));
        }

        history.borrow_mut().fail_replace = false;
        let retry = fixture.retry_correction.get().unwrap();
        dom.in_scope(ScopeId::APP, || retry.call(()));
        settle_users_page(&mut dom);
        assert_eq!(history.borrow().replace_attempts, 2);
        assert_eq!(history.borrow().index, 1);
        assert_eq!(state(), UsersQuery { page: 1, ..a });
        assert_eq!(history.borrow().current_query(), state());
        assert_eq!(
            requests.borrow().as_slice(),
            &[
                (String::new(), 1),
                ("A".into(), 2),
                ("B".into(), 1),
                ("A".into(), 2),
                ("A".into(), 1),
            ],
        );

        dom.in_runtime(|| history.borrow_mut().navigate(2));
        settle_users_page(&mut dom);
        assert_eq!(state(), b);
        assert_eq!(history.borrow().current_query(), b);
        assert_eq!(requests.borrow().last(), Some(&("B".into(), 1)));
    }
}
