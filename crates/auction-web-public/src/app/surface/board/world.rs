//! Public 看板世界选择与世界标签辅助。

use super::*;

/// 公开端世界选择器；世界是否可用由服务端目录决定，切换后重新读取权威当前拍卖。
#[component]
pub(crate) fn WorldPicker(mut selected_world: Signal<String>) -> Element {
    let worlds = use_resource(fetch_worlds);
    let options = match &*worlds.read_unchecked() {
        Some(Ok(worlds)) => enabled_worlds(worlds),
        _ => vec!["main".to_owned(), "isekai".to_owned()],
    };
    let current = selected_world();
    let active = options
        .iter()
        .find(|world| **world == current)
        .cloned()
        .unwrap_or_else(|| options.first().cloned().unwrap_or_else(|| "main".to_owned()));
    let active_for_effect = active.clone();
    use_effect(move || {
        if selected_world() != active_for_effect {
            selected_world.set(active_for_effect.clone());
        }
    });
    let options_empty = options.is_empty();

    rsx! {
        label { class: "public-world-switcher",
            span { {t!("world-label")} }
            select {
                value: active.clone(),
                disabled: options_empty,
                onchange: move |event| {
                    let world = event.value();
                    selected_world.set(world.clone());
                    update_public_query(
                        |query| {
                            query.world = normalize_world_query(&world);
                            query.auction_id = None;
                        },
                        true,
                    );
                },
                for world in options {
                    option { value: world.clone(), {world_label(&world)} }
                }
            }
        }
    }
}

pub(crate) fn enabled_worlds(worlds: &Worlds) -> Vec<String> {
    worlds
        .worlds
        .iter()
        .filter(|(_, status)| status.enabled)
        .map(|(world, _): (&String, &WorldStatus)| world.clone())
        .collect()
}

pub(crate) fn world_label(world: &str) -> String {
    match world {
        "main" => t!("auction-world-main"),
        "isekai" => t!("auction-world-isekai"),
        _ => world.to_owned(),
    }
}
