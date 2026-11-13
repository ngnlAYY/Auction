//! 目录分组与等级筛选；草稿局部持有，加载或错误不卸载输入，查询由看板写回 URL。

use super::*;
use crate::api::CatalogLevelFilter;

/// 分组、包含式等级上限和无等级开关共用一行；只表达读取条件，不修改装备或竞价资格。
/// `children` 渲染在工具栏右端，供拍卖页放置预览与往期场次的视图切换。
#[component]
pub(crate) fn CatalogGroupFilters(
    active_equipment_group: String,
    max_level: Option<String>,
    include_unleveled: String,
    on_equipment_group_change: EventHandler<String>,
    on_level_change: EventHandler<CatalogLevelFilter>,
    children: Element,
) -> Element {
    let mut maximum = use_signal(|| max_level.clone().unwrap_or_default());
    let mut include = use_signal(|| include_unleveled == "true");
    let mut invalid = use_signal(|| {
        CatalogLevelFilter::parse(max_level.as_deref().unwrap_or_default(), &include_unleveled)
            .is_err()
    });
    // 只在 URL 条件改变时恢复草稿；同条件后台刷新不能打断正在输入的等级。
    use_effect(use_reactive(
        (&max_level, &include_unleveled),
        move |(maximum_value, include_value)| {
            let maximum_value = maximum_value.unwrap_or_default();
            invalid.set(CatalogLevelFilter::parse(&maximum_value, &include_value).is_err());
            maximum.set(maximum_value);
            include.set(include_value == "true");
        },
    ));
    let apply = EventHandler::new(move |include_unleveled: bool| {
        match CatalogLevelFilter::parse(
            &maximum(),
            if include_unleveled { "true" } else { "false" },
        ) {
            Ok(levels) => {
                invalid.set(false);
                on_level_change.call(levels);
            }
            Err(()) => invalid.set(true),
        }
    });

    rsx! {
        div { class: "public-auction-group-toolbar",
            // 桌面分组标签与手机原生选择器由 CSS 二选一，共用当前分组与回调。
            nav { class: "public-auction-filter public-desktop", aria_label: t!("filter-equipment-group-label"),
                span { class: "public-auction-filter__label", {t!("equipment-group")} }
                for equipment_group in ["all", "rare", "fusion"] {
                    button {
                        class: "public-auction-filter__tab",
                        r#type: "button",
                        aria_current: if active_equipment_group == equipment_group { "true" } else { "false" },
                        onclick: move |_| on_equipment_group_change.call(equipment_group.to_owned()),
                        if equipment_group == "all" { {t!("filter-all")} }
                        else { {equipment_group_label(equipment_group)} }
                    }
                }
            }
            presentation::mobile::EquipmentGroupFilter {
                active: active_equipment_group.clone(),
                on_change: on_equipment_group_change,
            }
            form {
                class: "public-catalog-level-filter",
                method: "get",
                action: current_public_pathname(),
                onsubmit: move |event| {
                    event.prevent_default();
                    apply.call(include());
                },
                div { class: "public-catalog-level-filter__controls",
                    label { class: "public-catalog-level-filter__maximum", r#for: "public-catalog-max-level",
                        span { {t!("filter-max-level")} }
                        input {
                            id: "public-catalog-max-level",
                            name: "max_level",
                            r#type: "text",
                            inputmode: "numeric",
                            enterkeyhint: "done",
                            autocomplete: "off",
                            spellcheck: "false",
                            maxlength: "19",
                            value: maximum(),
                            placeholder: t!("filter-level-unlimited"),
                            aria_invalid: invalid().to_string(),
                            "aria-description": t!("filter-level-hint"),
                            aria_describedby: if invalid() { "public-catalog-level-error" } else { "" },
                            oninput: move |event| {
                                maximum.set(event.value());
                                invalid.set(false);
                            },
                            onblur: move |_| apply.call(include()),
                        }
                    }
                    label { class: "public-catalog-level-filter__unleveled", r#for: "public-catalog-include-unleveled",
                        input {
                            id: "public-catalog-include-unleveled",
                            name: "include_unleveled",
                            r#type: "checkbox",
                            checked: include(),
                            "aria-description": t!("filter-level-hint"),
                            onchange: move |event| {
                                let checked = event.checked();
                                include.set(checked);
                                apply.call(checked);
                            },
                        }
                        span { {t!("filter-include-unleveled")} }
                    }
                }
                if invalid() {
                    p { id: "public-catalog-level-error", class: "public-catalog-level-filter__error", role: "alert", {t!("filter-level-invalid")} }
                }
            }
            {children}
        }
    }
}
