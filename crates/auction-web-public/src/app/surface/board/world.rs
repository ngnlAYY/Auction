//! Public 看板世界选择与世界标签辅助。

use super::*;

/// 公开端世界选择器；世界是否可用由服务端目录决定，切换后重新读取权威当前拍卖。
#[component]
pub(crate) fn WorldPicker(mut selected_world: Signal<String>) -> Element {
    let worlds = use_resource(fetch_worlds);
    let (options, worlds_loaded) = match &*worlds.read_unchecked() {
        Some(Ok(worlds)) => (header_worlds(worlds), true),
        _ => (vec!["main".to_owned(), "isekai".to_owned()], false),
    };
    let current = selected_world();
    let active = options.iter().find(|world| **world == current).cloned().unwrap_or_else(|| {
        // 旧版顶部选项沿用接口键顺序，但没有 URL 状态时仍以 Main 作为默认世界。
        options
            .iter()
            .find(|world| world.as_str() == "main")
            .cloned()
            .or_else(|| options.first().cloned())
            .unwrap_or_else(|| "main".to_owned())
    });
    let can_canonicalize = worlds_loaded && !options.is_empty();
    let active_for_effect = active.clone();
    use_effect(move || {
        if selected_world() != active_for_effect {
            selected_world.set(active_for_effect.clone());
            if !can_canonicalize {
                return;
            }
            // 旧版只有服务端目录使当前世界失效时才 replace URL；缺失或非法 world 只在内存回退 Main。
            let canonical_world = active_for_effect.clone();
            update_public_query(
                move |query| {
                    query.world = (canonical_world != "main").then_some(canonical_world);
                    query.auction_id = None;
                },
                true,
            );
        }
    });
    let options_empty = options.is_empty();

    rsx! {
        label { class: "public-world-switcher",
            span { {t!("world-label")} }
            select {
                id: "public-world",
                name: "world",
                value: active.clone(),
                disabled: options_empty,
                onchange: move |event| {
                    let world = event.value();
                    selected_world.set(world.clone());
                    update_public_query(
                        |query| {
                            // 主世界沿用旧版无参数 URL；用户主动切换要建立可返回的历史条目。
                            query.world = (world != "main").then(|| world.clone());
                        },
                        false,
                    );
                },
                for world in options {
                    option {
                        value: world.clone(),
                        selected: active == world,
                        {world_label(&world)}
                    }
                }
            }
        }
    }
}

fn header_worlds(worlds: &Worlds) -> Vec<String> {
    // 旧版顶部选择器直接沿用 worlds 对象的稳定键顺序，但只接受两个冻结世界。
    worlds
        .worlds
        .iter()
        .filter(|(world, status)| is_supported_world(world) && status.enabled)
        .map(|(world, _): (&String, &WorldStatus)| world.clone())
        .collect()
}

pub(crate) fn enabled_worlds(worlds: &Worlds) -> Vec<String> {
    let mut enabled = worlds
        .worlds
        .iter()
        .filter(|(world, status)| is_supported_world(world) && status.enabled)
        .map(|(world, _): (&String, &WorldStatus)| world.clone())
        .collect::<Vec<_>>();
    // 旧版页面筛选器固定以 Main 为首项。
    enabled.sort_by_key(|world| match world.as_str() {
        "main" => 0_u8,
        "isekai" => 1_u8,
        _ => 2_u8,
    });
    enabled
}

fn is_supported_world(world: &str) -> bool {
    matches!(world, "main" | "isekai")
}

pub(crate) fn world_label(world: &str) -> String {
    match world {
        "main" => t!("auction-world-main"),
        "isekai" => t!("auction-world-isekai"),
        _ => world.to_owned(),
    }
}
