//! 数据库权威业务世界状态控件。

use auction_web_shared::t;
use auction_web_shared::{AsyncState, AsyncStateKind, Dialog};
use dioxus::prelude::*;

use super::super::{admin_api_error_message, admin_world_label};
use crate::api::{
    AdminBusinessWorld, fetch_admin_business_worlds, set_admin_business_world_enabled,
};
use crate::surface::pending::{
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
    let pending = use_pending_commands_with_gate(refresh, Some(command_gate), |entity| {
        entity.starts_with("world:")
    });
    let worlds = use_resource(move || {
        let generation = refresh();
        async move {
            #[cfg(test)]
            if let Some(reads) = try_consume_context::<std::rc::Rc<tests::WorldReads>>() {
                return (generation, reads.fetch().await);
            }
            (generation, fetch_admin_business_worlds().await)
        }
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
                Some((_, Err(code))) => rsx! {
                    // 共享面板没有 disabled 态；繁忙时重试点击在回调里直接忽略。
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
                Some((loaded_generation, Ok(worlds))) => rsx! {
                    div { class: "admin-business-worlds__grid",
                        for world in worlds {
                            BusinessWorldCard {
                                key: "{world.id}:{world.policy_revision}",
                                world,
                                notice,
                                refresh,
                                loaded_generation,
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
    loaded_generation: u64,
) -> Element {
    let pending = use_context::<PendingCommands>();
    let target_enabled = !world.enabled;
    let world_id = world.id.clone();
    let world_name = admin_world_label(&world.id);
    let action_busy = pending.is_busy(&format!("world:{}", world.id));
    // 旧快照可展示。只有当前读取的策略版本才能开启或确认新的启停命令。
    let read_is_current = move || refresh() == loaded_generation;
    let blocked = !read_is_current()
        || pending.any_busy()
        || pending.contains(&format!("world:{}", world.id));
    let availability = availability_label(&world.availability);
    let action_label = if action_busy {
        t!("admin-action-working")
    } else if world.enabled {
        t!("admin-business-world-disable")
    } else {
        t!("admin-business-world-enable")
    };
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
                onclick: move |_| {
                    if !read_is_current()
                        || pending.any_busy() || pending.contains(&format!("world:{}", world.id)) {
                        return;
                    }
                    notice.set(None);
                    confirm_open.set(true);
                },
                "{action_label}"
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
                            world: world_name
                        )}
                    } else {
                        {t!(
                            "admin-business-world-enable-confirm-description",
                            world: world_name
                        )}
                    }
                }
                div { class: "admin-shell__dialog-actions",
                    button {
                        class: if world.enabled { "admin-btn-danger" } else { "admin-btn-primary" },
                        r#type: "button",
                        disabled: blocked,
                        onclick: move |_| {
                            if !read_is_current()
                                || pending.any_busy() || pending.contains(&format!("world:{world_id}")) {
                                return;
                            }
                            let request_world_id = world_id.clone();
                            let failure_world_id = world_id.clone();
                            let submitted = pending.submit(
                                format!("world:{world_id}"),
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

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::future::ready;
    use std::rc::Rc;

    use dioxus::prelude::*;

    use super::AdminBusinessWorlds;
    use crate::api::{AdminApiFailure, AdminBusinessWorld};
    use crate::surface::auctions::draft_page_tests::{
        RecordRead, button, click_button, has_live_text, mount_writer, settle_writer,
    };
    use crate::surface::pending::{
        PendingCommandHost, PendingCommandNotices, use_pending_commands,
    };

    pub(super) struct WorldReads {
        count: Cell<usize>,
        refreshed: RecordRead<Vec<AdminBusinessWorld>>,
    }

    impl WorldReads {
        pub(super) async fn fetch(&self) -> Result<Vec<AdminBusinessWorld>, String> {
            let previous = self.count.replace(self.count.get() + 1);
            if previous < 2 {
                return Ok(worlds(true, 7));
            }
            self.refreshed.fetch().await
        }
    }

    fn worlds(enabled: bool, policy_revision: i64) -> Vec<AdminBusinessWorld> {
        vec![AdminBusinessWorld {
            id: "main".to_owned(),
            enabled,
            availability: "available".to_owned(),
            policy_revision,
        }]
    }

    #[component]
    fn OriginalWorldCommand() -> Element {
        let reload = use_signal(|| 0);
        let pending = use_pending_commands(reload, |entity| entity == "world:main");
        let writes = use_context::<Rc<Cell<usize>>>();
        rsx! {
            button { onclick: move |_| {
                let writes = writes.clone();
                assert!(pending.submit(
                    "world:main".to_owned(), "关闭业务世界".to_owned(),
                    move |_| {
                        let previous = writes.replace(writes.get() + 1);
                        ready(if previous == 0 {
                            Err(AdminApiFailure::from_http_body(
                                503, r#"{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}"#,
                            ).code)
                        } else { Ok(()) })
                    }, |()| {}, |_| {},
                ));
            }, {"从原先的卡片发起命令".to_owned()} }
        }
    }

    #[test]
    fn reopened_world_card_awaits_a_current_policy_after_replay() {
        let reads = Rc::new(WorldReads { count: Cell::new(0), refreshed: RecordRead::new() });
        let (mut page, _) = mount_writer({
            let reads = reads.clone();
            move || {
                use_context_provider(|| reads.clone());
                use_context_provider(|| Rc::new(Cell::new(0_usize)));
                let host = use_context::<PendingCommandHost>();
                let mut visible = use_signal(|| true);
                let mut original = use_signal(|| true);
                rsx! {
                    button { onclick: move |_| { visible.toggle(); original.set(false); }, {"切换测试路由".to_owned()} }
                    PendingCommandNotices { host }
                    if visible() {
                        if original() { OriginalWorldCommand {} }
                        AdminBusinessWorlds {}
                    }
                }
            }
        });
        assert!(button(&page, "关闭业务世界").is_some_and(|(_, disabled)| !disabled));
        click_button(&mut page, "从原先的卡片发起命令");
        click_button(&mut page, "切换测试路由");
        click_button(&mut page, "切换测试路由");
        assert_eq!(reads.count.get(), 2);
        assert!(button(&page, "原样重试").is_some());
        click_button(&mut page, "原样重试");
        assert_eq!(reads.count.get(), 3);
        let (open, disabled) = button(&page, "关闭业务世界").unwrap();
        assert!(disabled);
        page.dispatch("click", open, "");
        assert!(!has_live_text(&page, "确认关闭业务世界"));
        reads.refreshed.respond(Ok(worlds(false, 8)));
        settle_writer(&mut page);
        click_button(&mut page, "开放业务世界");
        assert!(has_live_text(&page, "确认开放业务世界"));
    }
}
