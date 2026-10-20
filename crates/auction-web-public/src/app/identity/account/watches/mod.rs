//! “我的”当前场次标记列表；账号、世界与场次各有独立资源边界。

mod load;
mod table;

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::{PublicSession, Worlds, fetch_worlds};
use crate::app::surface::{PublicSessionResource, supported_worlds, world_label};

fn current_worlds(worlds: &Worlds) -> Vec<String> {
    supported_worlds(worlds)
        .into_iter()
        .filter(|world| worlds.worlds.get(world).is_some_and(|status| status.enabled))
        .collect()
}

fn identity_matches(session: PublicSessionResource, user_id: &str) -> bool {
    snapshot_matches(&session.read(), user_id)
}

// 异步读取前后只复核当前身份，不能把会话续期时间等非身份变化订阅成目录重读依赖。
fn current_identity_matches(session: PublicSessionResource, user_id: &str) -> bool {
    snapshot_matches(&session.peek(), user_id)
}

fn snapshot_matches(
    snapshot: &Option<Result<Option<PublicSession>, String>>,
    user_id: &str,
) -> bool {
    snapshot.as_ref().is_some_and(|snapshot| {
        snapshot
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .is_some_and(|identity| identity.authenticated && identity.user_id == user_id)
    })
}

/// 已认证账号的独立列表宿主，不随竞价和代理分页卸载。
#[component]
pub(super) fn CurrentWatches(user_id: String) -> Element {
    let session = use_context::<PublicSessionResource>();
    let refresh = use_context::<Signal<u64>>();
    let mut worlds = use_resource(move || {
        let _ = refresh();
        fetch_worlds()
    });
    if !identity_matches(session, &user_id) {
        return rsx! {};
    }
    rsx! {
        section { class: "public-account-section", aria_labelledby: "account-watches-title",
            h2 { id: "account-watches-title", {t!("account-watches-title")} }
            // 同一账号的后台世界目录重读保留已确认宿主，避免卸载世界列表及其分页。
            match &*worlds.read() {
                    None => rsx! {
                        p { class: "public-page-status", role: "status", {t!("account-watches-loading")} }
                    },
                    Some(Ok(worlds)) => rsx! {
                        if current_worlds(worlds).is_empty() {
                            p { class: "public-account-empty", {t!("account-watches-no-current")} }
                        }
                        for world in current_worlds(worlds) {
                            WorldWatches { key: "{user_id}:{world}", user_id: user_id.clone(), world }
                        }
                    },
                    _ => rsx! {
                        div { role: "alert",
                            p { {t!("account-watches-error")} }
                            button { r#type: "button", onclick: move |_| worlds.restart(), {t!("data-retry")} }
                        }
                    },
            }
        }
    }
}

#[component]
fn WorldWatches(user_id: String, world: String) -> Element {
    let session = use_context::<PublicSessionResource>();
    let refresh = use_context::<Signal<u64>>();
    let requested_user = user_id.clone();
    let requested_world = world.clone();
    let mut data = use_resource(move || {
        let _ = refresh();
        let user_id = requested_user.clone();
        let world = requested_world.clone();
        async move {
            let mut result = if current_identity_matches(session, &user_id) {
                load::load(&user_id, &world).await
            } else {
                Err(())
            };
            if !current_identity_matches(session, &user_id) {
                result = Err(());
            }
            result
        }
    });
    // 此列表不展示价格或领先者；竞价通知不改变这四列，生命周期/标记变更仍通过全局修订重读。
    if !identity_matches(session, &user_id) {
        return rsx! {};
    }
    let updating = *data.state().read() != UseResourceState::Ready;
    let snapshot = data.read();
    // Resource 保留上次完成值；同一身份和世界刷新时继续显示它，结果返回后再按场次 key 替换。
    let current = snapshot.as_ref();
    rsx! {
        section { class: "public-account-section", aria_label: world_label(&world), aria_busy: updating.to_string(),
            match current {
                None => rsx! {
                    h3 { {world_label(&world)} }
                    p { class: "public-page-status", role: "status", {t!("account-watches-loading")} }
                },
                Some(Err(_)) => rsx! {
                    h3 { {world_label(&world)} }
                    div { role: "alert",
                        p { {t!("account-watches-error")} }
                        button { r#type: "button", onclick: move |_| data.restart(), {t!("data-retry")} }
                    }
                },
                Some(Ok(None)) => rsx! {
                    h3 { {world_label(&world)} }
                    p { class: "public-account-empty", {t!("account-watches-no-current")} }
                },
                Some(Ok(Some(watched))) => rsx! {
                    table::WatchedTable {
                        key: "{user_id}:{world}:{watched.auction.id}",
                        watched: watched.clone(),
                    }
                },
            }
        }
    }
}
