//! Admin 用户筛选、列表和分页。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::AdminUserStatusFilter;
use crate::app::surface::AdminActionFeedback;

use super::super::pagination::AdminPager;
use super::create::AdminUserGeneratedKey;
use super::dialogs::{parse_status_filter, status_filter_value};
use super::row::AdminUserRow;

#[component]
pub(super) fn AdminUserTable(
    page: crate::api::AdminUserPage,
    mut name_filter: Signal<String>,
    mut name_draft: Signal<String>,
    mut status_filter: Signal<AdminUserStatusFilter>,
    mut page_number: Signal<i64>,
    reload_generation: Signal<u64>,
    action_busy: Signal<bool>,
    action_feedback: Signal<Option<AdminActionFeedback>>,
    create_notice: Signal<Option<String>>,
    on_generated_key: EventHandler<AdminUserGeneratedKey>,
) -> Element {
    rsx! {
        form {
            class: "admin-users-filter",
            onsubmit: move |event| {
                event.prevent_default();
                name_filter.set(name_draft().trim().to_owned());
                page_number.set(1);
            },
            label {
                r#for: "user-name-filter",
                {t!("admin-users-filter-name")}
                input {
                    id: "user-name-filter",
                    maxlength: "256",
                    value: name_draft(),
                    oninput: move |event| {
                        name_draft.set(event.value());
                    },
                }
            }
            label {
                r#for: "user-status-filter",
                {t!("admin-users-filter-status")}
                select {
                    id: "user-status-filter",
                    value: status_filter_value(status_filter()),
                    onchange: move |event| {
                        status_filter.set(parse_status_filter(&event.value()));
                        page_number.set(1);
                    },
                    option { value: "all", {t!("admin-users-status-all")} }
                    option { value: "active", {t!("admin-users-status-active")} }
                    option { value: "blacklisted", {t!("admin-users-status-blacklisted")} }
                }
            }
        }
        div { class: "admin-users-table-wrap",
            table { class: "admin-users-table",
                thead { tr {
                    th { {t!("admin-users-username")} }
                    th { {t!("admin-users-forum-account-id")} }
                    th { {t!("admin-users-status")} }
                    th { {t!("admin-users-blacklist-history")} }
                    th { {t!("admin-users-blacklist-count")} }
                    th { {t!("admin-users-created-at")} }
                    th { {t!("admin-users-actions")} }
                } }
                tbody {
                    for user in page.users.iter().cloned() {
                        AdminUserRow {
                            key: "{user.id}",
                            user,
                            reload_generation,
                            busy: action_busy,
                            feedback: action_feedback,
                            on_generated_key,
                        }
                    }
                }
            }
        }
        if page.users.is_empty() {
            p { class: "admin-shell__status", role: "status", {t!("admin-users-no-results")} }
        }
        if let Some(feedback) = action_feedback() {
            p {
                class: if feedback.is_success() {
                    "admin-shell__inline-ok"
                } else {
                    "admin-shell__inline-error"
                },
                role: if feedback.is_success() { "status" } else { "alert" },
                {feedback.message(t!("admin-users-updated"))}
            }
        }
        if let Some(notice) = create_notice() {
            p { class: "admin-shell__inline-ok", role: "status", {notice} }
        }
        AdminPager {
            page: page.pagination.page,
            page_size: page.pagination.page_size,
            total: page.pagination.total,
            label: t!("page-users-title"),
            on_page_change: move |next_page| page_number.set(next_page),
        }
    }
}
