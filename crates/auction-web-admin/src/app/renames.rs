//! Admin 改名历史页面。
//!
//! 当前页面只读取不可变更名事实；回滚操作等用户状态命令完成应用端口和版本 fence 后再
//! 添加，避免浏览器自行推导身份状态迁移。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{AdminRename, AdminRenamePage, fetch_admin_renames};

use super::AdminPanel;

pub(super) fn page() -> Element {
    let mut user_id = use_signal(|| "all".to_owned());
    let mut name = use_signal(String::new);
    let mut active_user_id = use_signal(|| "all".to_owned());
    let mut active_name = use_signal(String::new);
    let mut page_number = use_signal(|| 1_i64);
    let mut refresh = use_signal(|| 0_u64);
    let renames = use_resource(move || {
        let _ = refresh();
        fetch_admin_renames(active_user_id(), active_name(), page_number())
    });

    let load = move |_| {
        let next_user_id = user_id().trim().to_owned();
        let next_name = name().trim().to_owned();
        if next_user_id.is_empty() {
            return;
        }
        active_user_id.set(next_user_id);
        active_name.set(next_name);
        page_number.set(1);
        refresh.set(refresh().saturating_add(1));
    };

    match &*renames.read_unchecked() {
        None => rsx! {
            AdminPanel { title: t!("page-renames-title"), detail: t!("admin-renames-loading") }
        },
        Some(Err(_)) => rsx! {
            AdminPanel { title: t!("page-renames-title"), detail: t!("admin-renames-error") }
        },
        Some(Ok(page)) => rsx! {
            section { class: "admin-panel", aria_labelledby: "admin-renames-title",
                h1 { id: "admin-renames-title", {t!("page-renames-title")} }
                p { class: "admin-panel__note", {t!("page-renames-note")} }
                div { class: "admin-panel__actions",
                    label {
                        {t!("admin-renames-user-filter")}
                        input {
                            maxlength: "64",
                            value: user_id(),
                            oninput: move |event| user_id.set(event.value()),
                        }
                    }
                    label {
                        {t!("admin-renames-name-filter")}
                        input {
                            maxlength: "256",
                            value: name(),
                            oninput: move |event| name.set(event.value()),
                        }
                    }
                    button { r#type: "button", onclick: load, {t!("admin-renames-load")} }
                }
                RenameTable { page: page.clone() }
            }
        },
    }
}

#[component]
fn RenameTable(page: AdminRenamePage) -> Element {
    rsx! {
        section { class: "admin-panel", aria_labelledby: "admin-renames-table-title",
            h2 { id: "admin-renames-table-title", {t!("admin-renames-total", total: page.total)} }
            div { class: "public-auction-catalog",
                table {
                    thead {
                        tr {
                            th { {t!("admin-renames-id")} }
                            th { {t!("admin-renames-user")} }
                            th { {t!("admin-renames-previous")} }
                            th { {t!("admin-renames-next")} }
                            th { {t!("admin-renames-actor")} }
                            th { {t!("admin-renames-created")} }
                            th { {t!("admin-renames-reverted")} }
                        }
                    }
                    tbody {
                        for rename in page.renames {
                            RenameRow { rename }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn RenameRow(rename: AdminRename) -> Element {
    rsx! {
        tr {
            td { "{rename.id}" }
            td { "{rename.user_id}" }
            td { "{rename.previous_username}" }
            td { "{rename.next_username}" }
            td { "{rename.actor}" }
            td { time { datetime: rename.created_at.clone(), "{rename.created_at}" } }
            td {
                if let Some(reverted_at) = rename.reverted_at {
                    time { datetime: reverted_at.clone(), "{reverted_at}" }
                } else {
                    {t!("value-empty")}
                }
            }
        }
    }
}
