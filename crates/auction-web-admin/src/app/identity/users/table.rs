//! Admin 用户筛选、列表和分页。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::AdminUserStatusFilter;

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
    on_generated_key: EventHandler<AdminUserGeneratedKey>,
) -> Element {
    rsx! {
        section { class: "admin-panel", aria_labelledby: "admin-users-title",
            p { class: "admin-panel__note", {t!("admin-users-summary", total: page.pagination.total)} }
            form {
                class: "admin-panel__actions",
                onsubmit: move |event| {
                    event.prevent_default();
                    name_filter.set(name_draft().trim().to_owned());
                    page_number.set(1);
                },
                label {
                    {t!("admin-users-filter-name")}
                    input {
                        value: name_draft(),
                        oninput: move |event| {
                            name_draft.set(event.value());
                        },
                    }
                }
                label {
                    {t!("admin-users-filter-status")}
                    select {
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
            if page.users.is_empty() {
                p { class: "admin-panel__note", role: "status", {t!("admin-users-no-results")} }
            } else {
                div { class: "public-auction-catalog admin-users__table-wrap",
                    table {
                        thead { tr {
                            th { {t!("admin-users-username")} }
                            th { {t!("admin-users-game-player-id")} }
                            th { {t!("admin-users-status")} }
                            th { {t!("admin-users-blacklist-history")} }
                            th { {t!("admin-users-blacklist-count")} }
                            th { {t!("admin-users-created-at")} }
                            th { {t!("admin-users-key-status")} }
                            th { {t!("admin-users-version")} }
                            th { {t!("admin-users-actions")} }
                        } }
                        tbody {
                            for user in page.users {
                                AdminUserRow { user, reload_generation, on_generated_key }
                            }
                        }
                    }
                }
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
}
