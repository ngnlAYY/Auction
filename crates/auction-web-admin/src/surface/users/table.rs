//! Admin 用户筛选、列表和分页。

use std::cmp::Ordering;

use auction_web_shared::t;
use auction_web_shared::{AsyncState, AsyncStateKind};
use dioxus::prelude::*;

use super::dialogs::{parse_status_filter, status_filter_value};
use super::fields::{UserFieldError, UserFieldIssue, search_name_issue};
use super::row::AdminUserRow;
use crate::api::{AdminUser, AdminUserStatusFilter};
use crate::surface::AdminActionFeedback;
use crate::surface::pagination::{AdminPager as Pager, AdminPagination};
use crate::surface::sort::{SortDirection, SortableTh, sort_rows};
use crate::surface::validation::focus_field;

#[component]
pub(super) fn AdminUserFilters(
    applied: Memo<(String, AdminUserStatusFilter)>,
    history_revision: Signal<u64>,
    on_name_apply: EventHandler<String>,
    on_status_change: EventHandler<AdminUserStatusFilter>,
) -> Element {
    let draft = use_search_draft(applied, history_revision);
    let mut name_focused = use_signal(|| false);
    let status = applied.read().1;
    let issue = (draft.issue)();
    let announced = (draft.announcement)().issue;
    let described_by = if announced.is_some() {
        Some("user-name-filter-announcement")
    } else {
        issue.map(|_| "user-name-filter-error")
    };
    rsx! {
        form {
            class: "admin-users-filter",
            role: "search",
            aria_label: t!("admin-users-filter-apply"),
            method: "get",
            action: "/users",
            autocomplete: "off",
            onsubmit: move |event| {
                event.prevent_default();
                if let Some(name) = draft.validate_submission(name_focused()) {
                    on_name_apply.call(name);
                } else if !name_focused() {
                    focus_field("user-name-filter");
                }
            },
            label {
                r#for: "user-name-filter",
                {t!("admin-users-filter-name")}
                input {
                    id: "user-name-filter",
                    name: "name",
                    r#type: "search",
                    autocapitalize: "none",
                    spellcheck: "false",
                    enterkeyhint: "search",
                    aria_invalid: issue.is_some().to_string(),
                    aria_describedby: described_by,
                    value: (draft.name)(),
                    onfocus: move |_| name_focused.set(true),
                    onblur: move |_| name_focused.set(false),
                    oninput: move |event| draft.edit(event.value()),
                }
            }
            label {
                r#for: "user-status-filter",
                {t!("admin-users-filter-status")}
                select {
                    id: "user-status-filter",
                    name: "status",
                    value: status_filter_value(status),
                    onchange: move |event| {
                        on_status_change.call(parse_status_filter(&event.value()));
                    },
                    option { value: "all", {t!("admin-users-status-all")} }
                    option { value: "active", {t!("admin-users-status-active")} }
                    option { value: "blacklisted", {t!("admin-users-status-blacklisted")} }
                }
            }
            button { r#type: "submit", class: "admin-btn-secondary", {t!("admin-users-filter-apply")} }
        }
        UserFieldError { id: "user-name-filter-error", issue: issue.filter(|_| announced.is_none()) }
        UserSearchAnnouncement { announcement: draft.announcement }
    }
}

#[component]
fn UserSearchAnnouncement(announcement: Signal<SearchAnnouncement>) -> Element {
    let SearchAnnouncement { revision, issue } = announcement();
    rsx! {
        div {
            id: "user-name-filter-announcement",
            aria_live: "polite",
            aria_atomic: "true",
            class: issue.map(|_| "admin-field-error"),
            // 同一错误再次提交也产生新增文本节点，常驻播报区与字段焦点保持稳定。
            for issue in issue.into_iter() {
                span { key: "{revision}", {issue.message()} }
            }
        }
    }
}

#[derive(Clone, Copy, Default)]
struct SearchAnnouncement {
    revision: u64,
    issue: Option<UserFieldIssue>,
}

#[derive(Clone, Copy)]
struct UserSearchDraft {
    name: Signal<String>,
    issue: Signal<Option<UserFieldIssue>>,
    announcement: Signal<SearchAnnouncement>,
}

impl UserSearchDraft {
    fn edit(mut self, name: String) {
        self.announcement.write().issue = None;
        if self.issue.peek().is_some() {
            self.issue.set(search_name_issue(&name));
        }
        self.name.set(name);
    }

    fn validate_submission(mut self, focused: bool) -> Option<String> {
        let name = self.name.peek().clone();
        let issue = search_name_issue(&name);
        self.issue.set(issue);
        // Enter 提交时焦点可能已经在错误字段上，重新聚焦不会播报。
        let mut announcement = self.announcement.write();
        announcement.issue = issue.filter(|_| focused);
        if announcement.issue.is_some() {
            announcement.revision = announcement.revision.wrapping_add(1);
        }
        issue.is_none().then_some(name)
    }
}

fn use_search_draft(
    applied: Memo<(String, AdminUserStatusFilter)>,
    history_revision: Signal<u64>,
) -> UserSearchDraft {
    let mut draft = UserSearchDraft {
        name: use_signal(|| applied.peek().0.clone()),
        issue: use_signal(|| None),
        announcement: use_signal(SearchAnnouncement::default),
    };
    use_effect(move || {
        let _ = history_revision();
        draft.name.set(applied.read().0.clone());
        draft.issue.set(None);
        draft.announcement.write().issue = None;
    });
    draft
}

#[component]
pub(super) fn AdminUserTable(
    page: crate::api::AdminUserPage,
    pagination: AdminPagination,
    sort: String,
    direction: SortDirection,
    on_page_change: EventHandler<i64>,
    on_sort_change: EventHandler<(String, SortDirection)>,
    reload_generation: Signal<u64>,
    loaded_generation: u64,
    action_busy: Signal<bool>,
    action_feedback: Signal<Option<AdminActionFeedback>>,
) -> Element {
    let is_empty = page.users.is_empty();
    let mut users = page.users;
    sort_users(&mut users, &sort, direction);
    rsx! {
        div { class: "admin-users-table-wrap",
            table { class: "admin-users-table",
                aria_label: t!("page-users-title"),
                // 列序：名称、论坛 ID、状态、封禁理由、次数、注册时间、操作；为四个操作按钮预留同排空间。
                auction_web_shared::TableColumns { layout: "admin-users", widths: vec![12, 10, 6, 14, 5, 13, 40] }
                thead { tr {
                    SortableTh {
                        column: "username",
                        label: t!("admin-users-username"),
                        active: sort.clone(),
                        direction,
                        on_sort: on_sort_change,
                    }
                    th { scope: "col", {t!("admin-users-forum-account-id")} }
                    SortableTh {
                        column: "status",
                        label: t!("admin-users-status"),
                        active: sort.clone(),
                        direction,
                        on_sort: on_sort_change,
                    }
                    th { scope: "col", {t!("admin-users-blacklist-history")} }
                    SortableTh {
                        column: "blacklist_count",
                        label: t!("admin-users-blacklist-count"),
                        active: sort.clone(),
                        direction,
                        on_sort: on_sort_change,
                    }
                    SortableTh {
                        column: "created_at",
                        label: t!("admin-users-created-at"),
                        active: sort,
                        direction,
                        on_sort: on_sort_change,
                    }
                    th { scope: "col", {t!("admin-users-actions")} }
                } }
                tbody {
                    for user in users {
                        AdminUserRow {
                            key: "{user.id}",
                            user,
                            reload_generation,
                            loaded_generation,
                            busy: action_busy,
                            feedback: action_feedback,
                        }
                    }
                }
            }
        }
        if is_empty {
            AsyncState {
                kind: AsyncStateKind::Empty,
                title: t!("admin-users-no-results"),
                detail: "",
            }
        }
        Pager {
            pagination,
            page: page.pagination.page,
            page_size: page.pagination.page_size,
            total: Some(page.pagination.total),
            label: t!("page-users-title"),
            on_page_change,
        }
    }
}

/// 对当前页用户行排序；`column` 为空或不在允许集合时保持服务端原始顺序。
fn sort_users(users: &mut [AdminUser], column: &str, direction: SortDirection) {
    sort_rows(users, column, direction, |left, right, column| match column {
        "username" => left.username.cmp(&right.username),
        "status" => left.status.cmp(&right.status),
        "blacklist_count" => left.blacklist_count.cmp(&right.blacklist_count),
        "created_at" => left.created_at.cmp(&right.created_at),
        _ => Ordering::Equal,
    });
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use auction_web_shared::i18n::{DEFAULT_ADMIN_LOCALE, build_bundle};
    use dioxus::dioxus_core::{Mutation, Mutations, NoOpMutations};
    use dioxus::prelude::*;
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;

    use super::{UserSearchAnnouncement, UserSearchDraft, use_search_draft};
    use crate::api::AdminUserStatusFilter;
    use crate::surface::ADMIN_LOCALE_CATALOG;
    use crate::surface::users::fields::UserFieldIssue;

    type Query = (&'static str, AdminUserStatusFilter, i64);

    #[derive(Default)]
    struct FilterHarness {
        query: Cell<Option<Signal<Query>>>,
        history: Cell<Option<Signal<u64>>>,
        draft: Cell<Option<UserSearchDraft>>,
    }

    fn query_page() -> Element {
        try_use_init_i18n_from_bundle(|| {
            build_bundle(
                &DEFAULT_ADMIN_LOCALE,
                ADMIN_LOCALE_CATALOG.default_ftl,
                &DEFAULT_ADMIN_LOCALE,
                None,
            )
        })
        .unwrap();
        let control = consume_context::<Rc<FilterHarness>>();
        let query = use_signal(|| ("玩家甲", AdminUserStatusFilter::Active, 1));
        let history = use_signal(|| 0_u64);
        let applied = use_memo(move || {
            let (name, status, _) = query();
            (name.to_owned(), status)
        });
        let draft = use_search_draft(applied, history);
        control.query.set(Some(query));
        control.history.set(Some(history));
        control.draft.set(Some(draft));
        rsx! {
            input { value: (draft.name)() }
            UserSearchAnnouncement { announcement: draft.announcement }
        }
    }

    #[test]
    fn navigation_restores_applied_filter_but_paging_keeps_unsubmitted_input() {
        let control = Rc::new(FilterHarness::default());
        let mut dom = VirtualDom::new(query_page).with_root_context(control.clone());
        dom.rebuild_in_place();
        dom.process_events();
        let mut query = control.query.get().unwrap();
        let UserSearchDraft { name: mut input, mut issue, .. } = control.draft.get().unwrap();

        dom.in_runtime(|| {
            input.set("未提交的内容".to_owned());
            issue.set(Some(UserFieldIssue::Name));
            query.set(("玩家甲", AdminUserStatusFilter::Active, 2));
        });
        dom.render_immediate(&mut NoOpMutations);
        dom.process_events();
        assert_eq!(&*input.peek(), "未提交的内容");
        assert_eq!(*issue.peek(), Some(UserFieldIssue::Name));

        let mut history = control.history.get().unwrap();
        dom.in_runtime(|| {
            query.set(("玩家甲", AdminUserStatusFilter::Active, 1));
            history.set(1);
        });
        dom.render_immediate(&mut NoOpMutations);
        dom.process_events();
        assert_eq!(&*input.peek(), "玩家甲");
        assert!(issue.peek().is_none());

        for (expected, status) in [
            ("玩家乙", AdminUserStatusFilter::Blacklisted),
            ("玩家甲", AdminUserStatusFilter::Active),
            ("玩家乙", AdminUserStatusFilter::Blacklisted),
        ] {
            dom.in_runtime(|| query.set((expected, status, 1)));
            dom.render_immediate(&mut NoOpMutations);
            dom.process_events();
            assert_eq!(&*input.peek(), expected);
            assert!(issue.peek().is_none());
        }
    }

    #[test]
    fn invalid_submit_announces_only_when_the_search_already_has_focus() {
        let control = Rc::new(FilterHarness::default());
        let mut dom = VirtualDom::new(query_page).with_root_context(control.clone());
        dom.rebuild_in_place();
        dom.process_events();
        let draft = control.draft.get().unwrap();
        dom.in_runtime(|| {
            draft.edit(" 玩家".to_owned());
            assert_eq!(draft.validate_submission(false), None);
            assert_eq!(*draft.issue.peek(), Some(UserFieldIssue::Name));
            assert!(draft.announcement.peek().issue.is_none());
            assert_eq!(draft.validate_submission(true), None);
            assert_eq!(draft.announcement.peek().issue, Some(UserFieldIssue::Name));

            draft.edit(" 玩家甲".to_owned());
            assert!(draft.announcement.peek().issue.is_none());
            assert_eq!(*draft.issue.peek(), Some(UserFieldIssue::Name));
            draft.edit("玩家甲".to_owned());
            assert_eq!(draft.validate_submission(true).as_deref(), Some("玩家甲"));
            assert!(draft.issue.peek().is_none());
            assert!(draft.announcement.peek().issue.is_none());
        });
    }

    #[test]
    fn each_focused_failed_submit_updates_the_live_region() {
        let control = Rc::new(FilterHarness::default());
        let mut dom = VirtualDom::new(query_page).with_root_context(control.clone());
        dom.rebuild(&mut Mutations::default());
        dom.process_events();
        let draft = control.draft.get().unwrap();
        let message = dom.in_scope(ScopeId::APP, || UserFieldIssue::Name.message());
        dom.in_runtime(|| draft.edit(" 玩家".to_owned()));

        for attempt in 1..=2 {
            dom.in_runtime(|| assert_eq!(draft.validate_submission(true), None));
            let mut changes = Mutations::default();
            dom.render_immediate(&mut changes);
            assert!(
                changes.edits.iter().any(|edit| matches!(edit,
                    Mutation::CreateTextNode { value, .. } | Mutation::SetText { value, .. }
                    if value == &message
                )),
                "第 {attempt} 次提交应写入播报文字"
            );
        }

        dom.in_runtime(|| draft.edit(" 玩家甲".to_owned()));
        let mut changes = Mutations::default();
        dom.render_immediate(&mut changes);
        assert!(!changes.edits.iter().any(|edit| matches!(edit,
            Mutation::CreateTextNode { value, .. } | Mutation::SetText { value, .. }
            if value == &message
        )));
    }
}
