//! 数据库权威业务世界状态控件。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::{
    AdminBusinessWorld, fetch_admin_business_worlds, set_admin_business_world_enabled,
};

use auction_web_shared::{AsyncState, AsyncStateKind, Dialog};

use super::super::{admin_api_error_message, admin_world_label};
use crate::app::surface::pending::{
    PendingCommandPanel, PendingCommands, set_if_alive, use_pending_command_gate,
    use_pending_commands_with_gate,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct BusinessWorldNotice {
    world_id: String,
    enabled: bool,
    error_code: Option<String>,
}

#[component]
pub(super) fn AdminBusinessWorlds() -> Element {
    let mut refresh = use_signal(|| 0_u64);
    let notice = use_signal(|| None::<BusinessWorldNotice>);
    let command_gate = use_pending_command_gate();
    let pending = use_pending_commands_with_gate(refresh, Some(command_gate));
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
            PendingCommandPanel { owner: pending }
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
                            if !pending.any_busy() {
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
                                notice,
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
                        {t!("admin-business-world-enabled-success", world: admin_world_label(&current.world_id))}
                    } else {
                        {t!("admin-business-world-disabled-success", world: admin_world_label(&current.world_id))}
                    }
                }
            }
        }
    }
}

#[component]
fn BusinessWorldCard(
    world: AdminBusinessWorld,
    notice: Signal<Option<BusinessWorldNotice>>,
    refresh: Signal<u64>,
) -> Element {
    let pending = use_context::<PendingCommands>();
    let target_enabled = !world.enabled;
    let world_id = world.id.clone();
    let world_name = admin_world_label(&world.id);
    let action_busy = pending.is_busy(&world.id);
    let blocked = pending.any_busy() || pending.contains(&world.id);
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
                disabled: blocked,
                aria_busy: action_busy.to_string(),
                // 危险操作不直接生效：先清掉旧提示并打开确认对话框。
                onclick: move |_| {
                    if pending.any_busy() || pending.contains(&world.id) {
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
                        disabled: blocked,
                        onclick: move |_| {
                            if pending.any_busy() || pending.contains(&world_id) {
                                return;
                            }
                            let request_world_id = world_id.clone();
                            let failure_world_id = world_id.clone();
                            let submitted = pending.submit(
                                world_id.clone(),
                                if target_enabled {
                                    t!("admin-business-world-enable")
                                } else {
                                    t!("admin-business-world-disable")
                                },
                                move |command_id| {
                                    let world_id = request_world_id.clone();
                                    async move {
                                        set_admin_business_world_enabled(
                                            command_id,
                                            world_id,
                                            target_enabled,
                                            world.policy_revision,
                                        ).await.map_err(|error| error.code)
                                    }
                                },
                                move |result| {
                                    set_if_alive(notice, Some(BusinessWorldNotice {
                                        world_id: result.id,
                                        enabled: result.enabled,
                                        error_code: None,
                                    }));
                                },
                                move |code| {
                                    let refresh_required = code.starts_with("stale_");
                                    set_if_alive(notice, Some(BusinessWorldNotice {
                                        world_id: failure_world_id.clone(),
                                        enabled: target_enabled,
                                        error_code: Some(code),
                                    }));
                                    if refresh_required {
                                        let mut refresh = refresh;
                                        refresh.set(refresh().saturating_add(1));
                                    }
                                },
                            );
                            if submitted {
                                confirm_open.set(false);
                                notice.set(None);
                            }
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

fn availability_label(availability: &str) -> String {
    match availability {
        "available" => t!("admin-business-world-availability-available"),
        "degraded" => t!("admin-business-world-availability-degraded"),
        "unavailable" => t!("admin-business-world-availability-unavailable"),
        _ => availability.to_owned(),
    }
}
