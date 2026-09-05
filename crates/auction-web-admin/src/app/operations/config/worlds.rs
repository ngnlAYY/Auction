//! 数据库权威业务世界状态控件。

use std::collections::HashMap;

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use uuid::Uuid;

use crate::api::{
    AdminBusinessWorld, fetch_admin_business_worlds, set_admin_business_world_enabled,
};

use auction_web_shared::{AsyncState, AsyncStateKind, Dialog};

use super::super::admin_api_error_message;
use crate::app::surface::stable_scoped_command_id;

#[derive(Debug, Clone, PartialEq, Eq)]
struct BusinessWorldNotice {
    world_id: String,
    enabled: bool,
    error_code: Option<String>,
}

#[component]
pub(super) fn AdminBusinessWorlds() -> Element {
    let mut refresh = use_signal(|| 0_u64);
    let busy_world = use_signal(|| None::<String>);
    let notice = use_signal(|| None::<BusinessWorldNotice>);
    let command_ids = use_signal(HashMap::<String, Uuid>::new);
    let worlds = use_resource(move || {
        let _ = refresh();
        fetch_admin_business_worlds()
    });

    let state = worlds.read_unchecked().clone();
    rsx! {
        section {
            class: "admin-business-worlds admin-panel",
            aria_labelledby: "admin-business-worlds-title",
            h2 { id: "admin-business-worlds-title", {t!("admin-business-worlds-title")} }
            p { class: "admin-panel__note", {t!("admin-business-worlds-note")} }
            match state {
                None => rsx! {
                    AsyncState {
                        kind: AsyncStateKind::Loading,
                        title: t!("admin-business-worlds-loading"),
                        detail: "",
                    }
                },
                Some(Err(code)) => rsx! {
                    // 共享面板没有 disabled 态；繁忙时重试点击在回调里直接忽略，行为不变。
                    AsyncState {
                        kind: AsyncStateKind::Error,
                        title: admin_api_error_message(&code),
                        detail: "",
                        retry_label: t!("admin-retry"),
                        on_retry: move |_| {
                            if busy_world().is_none() {
                                refresh.set(refresh().saturating_add(1));
                            }
                        },
                    }
                },
                Some(Ok(worlds)) => rsx! {
                    div { class: "admin-business-worlds__grid",
                        for world in worlds {
                            BusinessWorldCard {
                                key: "{world.id}",
                                world,
                                busy_world,
                                notice,
                                command_ids,
                                refresh,
                            }
                        }
                    }
                },
            }
            if let Some(current) = notice() {
                p {
                    class: if current.error_code.is_none() {
                        "admin-panel__note action-feedback action-feedback--success"
                    } else {
                        "admin-panel__note action-feedback action-feedback--error"
                    },
                    role: if current.error_code.is_none() { "status" } else { "alert" },
                    if let Some(code) = &current.error_code {
                        {admin_api_error_message(code)}
                    } else if current.enabled {
                        {t!("admin-business-world-enabled-success", world: world_label(&current.world_id))}
                    } else {
                        {t!("admin-business-world-disabled-success", world: world_label(&current.world_id))}
                    }
                }
            }
        }
    }
}

#[component]
fn BusinessWorldCard(
    world: AdminBusinessWorld,
    busy_world: Signal<Option<String>>,
    notice: Signal<Option<BusinessWorldNotice>>,
    command_ids: Signal<HashMap<String, Uuid>>,
    refresh: Signal<u64>,
) -> Element {
    let target_enabled = !world.enabled;
    let signature = format!("{}:{}:{target_enabled}", world.id, world.policy_revision);
    let world_id = world.id.clone();
    let world_name = world_label(&world.id);
    let action_busy = busy_world().as_deref() == Some(world.id.as_str());
    let any_busy = busy_world().is_some();
    let availability = availability_label(&world.availability);
    // 启停是危险操作：按钮只打开确认对话框，真正的 mutation 由对话框确认按钮触发。
    let mut confirm_open = use_signal(|| false);

    rsx! {
        article { class: "admin-business-world",
            div { class: "admin-business-world__heading",
                h3 { "{world_name}" }
                span {
                    class: if world.enabled {
                        "admin-config-fact-enabled"
                    } else {
                        "admin-config-fact-disabled"
                    },
                    if world.enabled {
                        {t!("admin-business-world-enabled")}
                    } else {
                        {t!("admin-business-world-disabled")}
                    }
                }
            }
            dl { class: "admin-business-world__facts",
                div {
                    dt { {t!("admin-business-world-availability")} }
                    dd { "{availability}" }
                }
                div {
                    dt { {t!("admin-business-world-revision")} }
                    dd { "{world.policy_revision}" }
                }
            }
            button {
                class: if world.enabled { "admin-btn-danger" } else { "admin-btn-primary" },
                r#type: "button",
                disabled: any_busy,
                aria_busy: action_busy.to_string(),
                // 危险操作不直接生效：先清掉旧提示并打开确认对话框。
                onclick: move |_| {
                    if busy_world().is_some() {
                        return;
                    }
                    notice.set(None);
                    confirm_open.set(true);
                },
                if action_busy {
                    {t!("admin-action-working")}
                } else if world.enabled {
                    {t!("admin-business-world-disable")}
                } else {
                    {t!("admin-business-world-enable")}
                }
            }
        }
        if confirm_open() {
            // 外壳（遮罩/标题栏/关闭守卫）由共享 Dialog 接管。
            Dialog {
                title: if world.enabled {
                    t!("admin-business-world-disable-confirm-title")
                } else {
                    t!("admin-business-world-enable-confirm-title")
                },
                close_label: t!("admin-users-action-cancel"),
                busy: action_busy,
                on_close: move |_| confirm_open.set(false),
                p { class: "admin-panel__note",
                    if world.enabled {
                        {t!(
                            "admin-business-world-disable-confirm-description",
                            world: world_name.clone()
                        )}
                    } else {
                        {t!(
                            "admin-business-world-enable-confirm-description",
                            world: world_name.clone()
                        )}
                    }
                }
                div { class: "admin-shell__dialog-actions",
                    button {
                        class: if world.enabled { "admin-btn-danger" } else { "admin-btn-primary" },
                        r#type: "button",
                        disabled: any_busy,
                        onclick: move |_| {
                            if busy_world().is_some() {
                                return;
                            }
                            confirm_open.set(false);
                            let command_id = stable_scoped_command_id(command_ids, &signature);
                            busy_world.set(Some(world_id.clone()));
                            notice.set(None);
                            let world_id = world_id.clone();
                            let signature = signature.clone();
                            spawn(async move {
                                match set_admin_business_world_enabled(
                                    command_id,
                                    world_id.clone(),
                                    target_enabled,
                                    world.policy_revision,
                                )
                                .await
                                {
                                    Ok(result) => {
                                        command_ids.write().remove(&signature);
                                        notice.set(Some(BusinessWorldNotice {
                                            world_id: result.id,
                                            enabled: result.enabled,
                                            error_code: None,
                                        }));
                                        refresh.set(refresh().saturating_add(1));
                                    }
                                    Err(error) => {
                                        let refresh_required = error.code.starts_with("stale_");
                                        notice.set(Some(BusinessWorldNotice {
                                            world_id,
                                            enabled: target_enabled,
                                            error_code: Some(error.code),
                                        }));
                                        if refresh_required {
                                            command_ids.write().remove(&signature);
                                            refresh.set(refresh().saturating_add(1));
                                        }
                                    }
                                }
                                busy_world.set(None);
                            });
                        },
                        if action_busy {
                            {t!("admin-action-working")}
                        } else {
                            {t!("admin-users-action-confirm")}
                        }
                    }
                    button {
                        class: "admin-btn-secondary",
                        r#type: "button",
                        disabled: action_busy,
                        onclick: move |_| confirm_open.set(false),
                        {t!("admin-users-action-cancel")}
                    }
                }
            }
        }
    }
}

fn world_label(world_id: &str) -> String {
    match world_id {
        "main" => t!("admin-runtime-world-main"),
        "isekai" => t!("admin-runtime-world-isekai"),
        _ => world_id.to_owned(),
    }
}

fn availability_label(availability: &str) -> String {
    match availability {
        "available" => t!("admin-business-world-availability-available"),
        "degraded" => t!("admin-business-world-availability-degraded"),
        "unavailable" => t!("admin-business-world-availability-unavailable"),
        _ => availability.to_owned(),
    }
}
