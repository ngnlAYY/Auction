//! Public 看板世界选择与世界标签辅助。

use crate::api::{WorldStatus, Worlds, fetch_worlds};
use crate::surface::router::Route;
use crate::surface::{PublicBoardQuery, update_public_query_state};
use auction_web_shared::{QueryWriteMode, t};
use dioxus::prelude::*;

/// 公开端世界选择器；世界是否可用由服务端目录决定，切换后重新读取权威当前拍卖。
/// URL 写回经外壳共享的 query hook 状态信号（context），保证 hook 缓存与地址栏一致，
/// 浏览器前进/后退时 hook 的相等判断才能正确触发恢复。
#[component]
pub(crate) fn WorldPicker(mut selected_world: Signal<String>) -> Element {
    let route = use_route::<Route>();
    let history_owns_world_filter =
        matches!(&route, Route::AuctionList {} | Route::BidLog {} | Route::History {});
    let shell_query_state = use_context::<Signal<PublicBoardQuery>>();
    let historical_url = history_owns_world_filter || shell_query_state().auction_id.is_some();
    let worlds = use_resource(fetch_worlds);
    let (mut options, worlds_loaded) = match &*worlds.read_unchecked() {
        Some(Ok(worlds)) => (header_worlds(worlds), true),
        _ => (vec!["main".to_owned(), "isekai".to_owned()], false),
    };
    let current = selected_world();
    // 关闭世界的 settled 深链仍显示其真实世界，但不提供新的当前世界入口。
    let historical_disabled_world = historical_url && !options.contains(&current);
    if historical_disabled_world {
        options.push(current.clone());
    }
    let active = options.iter().find(|world| **world == current).cloned().unwrap_or_else(|| {
        // 旧版顶部选项沿用接口键顺序，但没有 URL 状态时仍以 Main 作为默认世界。
        options
            .iter()
            .find(|world| world.as_str() == "main")
            .cloned()
            .or_else(|| options.first().cloned())
            .unwrap_or_else(|| "main".to_owned())
    });
    let can_canonicalize = worlds_loaded && !options.is_empty() && !historical_url;
    use_effect(use_reactive(
        (&worlds_loaded, &active, &can_canonicalize, &historical_url),
        move |(worlds_loaded, active, can_canonicalize, historical_url)| {
            if !worlds_loaded || historical_url || selected_world() == active {
                return;
            }
            selected_world.set(active.clone());
            if !can_canonicalize {
                return;
            }
            // 仅当前世界失效时纠偏；历史筛选与指定场次深链不得被目录开关抹除。
            update_public_query_state(
                shell_query_state,
                move |query| {
                    query.world = (active != "main").then_some(active);
                },
                QueryWriteMode::Replace,
            );
        },
    ));
    let options_empty = options.is_empty();

    rsx! {
        label { class: "public-world-switcher",
            span { {t!("world-label")} }
            select {
                id: "public-world",
                name: "world",
                value: active,
                disabled: options_empty,
                onchange: move |event| {
                    let world = event.value();
                    selected_world.set(world.clone());
                    if history_owns_world_filter {
                        return;
                    }
                    update_public_query_state(
                        shell_query_state,
                        |query| {
                            // 主世界沿用旧版无参数 URL；用户主动切换要建立可返回的历史条目。
                            query.world = (world != "main").then(|| world.clone());
                        },
                        QueryWriteMode::Push,
                    );
                },
                for world in options {
                    option {
                        value: world.clone(),
                        selected: active == world,
                        disabled: historical_disabled_world && world == current,
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

/// 历史筛选保留两个受支持世界；业务开关只隐藏当前/可操作世界，不抹除 settled 事实。
pub(crate) fn supported_worlds(worlds: &Worlds) -> Vec<String> {
    let mut supported = worlds
        .worlds
        .iter()
        .filter(|(world, _)| is_supported_world(world))
        .map(|(world, _): (&String, &WorldStatus)| world.clone())
        .collect::<Vec<_>>();
    // 旧版页面筛选器固定以 Main 为首项。
    supported.sort_by_key(|world| match world.as_str() {
        "main" => 0_u8,
        "isekai" => 1_u8,
        _ => 2_u8,
    });
    supported
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
