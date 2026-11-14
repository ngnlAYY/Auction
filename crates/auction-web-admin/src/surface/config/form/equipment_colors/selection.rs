//! 标色规则的原生复选框组；候选关联只影响展示，已选值由用户明确取消。

use crate::surface::config::model::RuntimeConfigForm;
use auction_web_shared::t;
use dioxus::prelude::*;

fn selected_values(value: &str) -> Vec<String> {
    value.lines().map(str::trim).filter(|value| !value.is_empty()).map(str::to_owned).collect()
}

/// 每次读取最新规则，避免连续勾选使用渲染时的旧快照覆盖已选项。
fn update_selection(
    mut form: Signal<RuntimeConfigForm>,
    index: usize,
    field: &str,
    option: Option<&str>,
    checked: bool,
) {
    let form_state = form.read();
    let Some(rule) = form_state.equipment_colors().get(index) else { return };
    let current = match field {
        "category" => &rule.category,
        "equipment_type" => &rule.equipment_type,
        "quality" => &rule.quality,
        "prefixes" => &rule.prefixes,
        "suffixes" => &rule.suffixes,
        _ => return,
    };
    let mut selected = selected_values(current);
    drop(form_state);
    if let Some(option) = option {
        selected.retain(|value| !value.eq_ignore_ascii_case(option));
        if checked {
            selected.push(option.to_owned());
        }
    } else {
        selected.clear();
    }
    // 词表只是候选，不是历史规则白名单。改分类不得删除另一维选择，尤其不能把精确种类
    // 清空成“任意”；未知、历史及暂不相容的选择都继续可见，直到用户手动取消。
    form.write().set_equipment_color_field(index, field, selected.join("\n"));
}

/// 复选框均有原生名称和可见标签；空过滤集合表示任意，词缀集合仍保留原有空值语义。
#[component]
pub(super) fn ColorMultiSelect(
    form: Signal<RuntimeConfigForm>,
    index: usize,
    field: &'static str,
    label: String,
    options: Vec<String>,
    value: String,
    disabled: bool,
    on_dirty: EventHandler<()>,
    #[props(default)] allow_any: bool,
    #[props(default)] legacy_exact_type: Option<String>,
    #[props(default)] material_names: Vec<String>,
) -> Element {
    let selected = selected_values(&value);
    let mut choices = options;
    for value in &selected {
        if !choices.iter().any(|option| option.eq_ignore_ascii_case(value)) {
            choices.push(value.clone());
        }
    }
    let name = format!("color-{field}-{index}");
    let hint_id = format!("{name}-hint");
    rsx! {
        fieldset { class: "admin-runtime-color-multi", disabled,
            legend { "{label}" }
            if allow_any {
                button {
                    r#type: "button",
                    disabled: disabled || selected.is_empty(),
                    onclick: move |_| {
                        update_selection(form, index, field, None, false);
                        on_dirty.call(());
                    },
                    {t!("admin-runtime-color-any")}
                }
            }
            div { class: "admin-runtime-color-multi__options",
                for (position, option) in choices.into_iter().enumerate() {
                    label { class: "admin-runtime-color-multi__option",
                        r#for: "{name}-{position}",
                        key: "{option}",
                        input {
                            id: "{name}-{position}",
                            name: name.clone(),
                            r#type: "checkbox",
                            value: option.clone(),
                            checked: selected.iter().any(|value| value.eq_ignore_ascii_case(&option)),
                            "aria-describedby": hint_id.clone(),
                            onchange: {
                                let option = option.clone();
                                move |event: FormEvent| {
                                    update_selection(form, index, field, Some(&option), event.checked());
                                    on_dirty.call(());
                                }
                            },
                        }
                        span {
                            if field == "equipment_type" && material_names.iter().any(|name| name.eq_ignore_ascii_case(&option)) {
                                if legacy_exact_type.as_deref()
                                    .is_some_and(|legacy| legacy.eq_ignore_ascii_case(&option))
                                {
                                    {t!("admin-runtime-color-material-legacy", material: option.clone())}
                                } else {
                                    {t!("admin-runtime-color-material-all", material: option.clone())}
                                }
                            } else {
                                "{option}"
                            }
                        }
                    }
                }
            }
            small { id: hint_id,
                if allow_any {
                    {t!("admin-runtime-color-filter-hint")}
                } else {
                    {t!("admin-runtime-color-multi-hint")}
                }
            }
        }
    }
}
