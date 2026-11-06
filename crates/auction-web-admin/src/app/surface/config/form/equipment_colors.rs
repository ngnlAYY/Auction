//! 论坛装备名称标色规则编辑器。

use super::*;

mod options;

use options::{equipment_kinds_for, prefixes_for, suffixes_for};

mod selection;

use selection::ColorMultiSelect;

#[component]
pub(super) fn EquipmentColorRulesField(
    form: Signal<RuntimeConfigForm>,
    disabled: bool,
    on_dirty: EventHandler<()>,
) -> Element {
    let Some(taxonomy) = form.read().equipment_taxonomy() else {
        return rsx! { p { role: "alert", {t!("admin-runtime-invalid-form")} } };
    };
    let form_state = form.read();
    let rules = form_state.equipment_colors();
    let last_index = rules.len().saturating_sub(1);
    rsx! {
        div { class: "admin-runtime-color-rules",
            p { class: "admin-runtime-section-note", {t!("admin-runtime-equipment-colors-note")} }
            for (index, rule) in rules.iter().enumerate() {
                div { class: "admin-runtime-color-rule", key: "color-rule-{index}",
                    div { class: "admin-runtime-color-rule__head",
                        strong { {t!("admin-runtime-color-rule", number: index + 1)} }
                        div { class: "admin-runtime-color-rule__actions",
                            button {
                                r#type: "button",
                                class: "admin-btn-secondary",
                                disabled: disabled || index == 0,
                                onclick: {
                                    let mut form = form;
                                    move |_| {
                                        form.write().move_equipment_color(index, -1);
                                        on_dirty.call(());
                                    }
                                },
                                {t!("admin-runtime-color-up")}
                            }
                            button {
                                r#type: "button",
                                class: "admin-btn-secondary",
                                disabled: disabled || index >= last_index,
                                onclick: {
                                    let mut form = form;
                                    move |_| {
                                        form.write().move_equipment_color(index, 1);
                                        on_dirty.call(());
                                    }
                                },
                                {t!("admin-runtime-color-down")}
                            }
                            button {
                                r#type: "button",
                                class: "admin-btn-secondary",
                                disabled,
                                onclick: {
                                    let mut form = form;
                                    move |_| {
                                        form.write().remove_equipment_color(index);
                                        on_dirty.call(());
                                    }
                                },
                                {t!("admin-runtime-color-remove")}
                            }
                        }
                    }
                    div { class: "admin-runtime-field-grid",
                        ColorMultiSelect {
                            form,
                            index,
                            field: "category",
                            label: t!("admin-runtime-color-category"),
                            options: taxonomy.categories.clone(),
                            value: rule.category.clone(),
                            allow_any: true,
                            disabled,
                            on_dirty,
                        }
                        ColorMultiSelect {
                            form,
                            index,
                            field: "equipment_type",
                            label: t!("admin-runtime-color-type"),
                            options: equipment_kinds_for(&taxonomy, &rule.category),
                            material_names: taxonomy.materials.iter().map(|option| option.name.clone()).collect(),
                            value: rule.equipment_type.clone(),
                            legacy_exact_type: rule.legacy_equipment_type.clone(),
                            allow_any: true,
                            disabled,
                            on_dirty,
                        }
                        ColorMultiSelect {
                            form,
                            index,
                            field: "quality",
                            label: t!("admin-runtime-color-quality"),
                            options: taxonomy.qualities.iter().map(|option| option.name.clone()).collect(),
                            value: rule.quality.clone(),
                            allow_any: true,
                            disabled,
                            on_dirty,
                        }
                        label { class: "admin-runtime-field",
                            span { {t!("admin-runtime-color-level-min")} }
                            input {
                                r#type: "number",
                                min: "0",
                                max: "1000",
                                disabled,
                                value: rule.level_min.clone(),
                                oninput: {
                                    let mut form = form;
                                    move |event: FormEvent| {
                                        form.write().set_equipment_color_field(
                                            index,
                                            "level_min",
                                            event.value(),
                                        );
                                        on_dirty.call(());
                                    }
                                },
                            }
                        }
                        label { class: "admin-runtime-field",
                            span { {t!("admin-runtime-color-level-max")} }
                            input {
                                r#type: "number",
                                min: "0",
                                max: "1000",
                                disabled,
                                value: rule.level_max.clone(),
                                oninput: {
                                    let mut form = form;
                                    move |event: FormEvent| {
                                        form.write().set_equipment_color_field(
                                            index,
                                            "level_max",
                                            event.value(),
                                        );
                                        on_dirty.call(());
                                    }
                                },
                            }
                        }
                    }
                    div { class: "admin-runtime-field-grid admin-runtime-field-grid--lists",
                        ColorMultiSelect {
                            form,
                            index,
                            field: "prefixes",
                            label: t!("admin-runtime-color-prefixes"),
                            options: prefixes_for(&taxonomy, &rule.category, &rule.equipment_type),
                            value: rule.prefixes.clone(),
                            disabled,
                            on_dirty,
                        }
                        ColorMultiSelect {
                            form,
                            index,
                            field: "suffixes",
                            label: t!("admin-runtime-color-suffixes"),
                            options: suffixes_for(&taxonomy, &rule.category, &rule.equipment_type),
                            value: rule.suffixes.clone(),
                            disabled,
                            on_dirty,
                        }
                    }
                    div { class: "admin-runtime-field-grid",
                        label { class: "admin-runtime-field",
                            span { {t!("admin-runtime-color-prefix")} }
                            input {
                                r#type: "color",
                                disabled,
                                value: rule.prefix_color.clone(),
                                oninput: {
                                    let mut form = form;
                                    move |event: FormEvent| {
                                        form.write().set_equipment_color_field(
                                            index,
                                            "prefix_color",
                                            event.value(),
                                        );
                                        on_dirty.call(());
                                    }
                                },
                            }
                        }
                        label { class: "admin-runtime-field",
                            span { {t!("admin-runtime-color-suffix")} }
                            input {
                                r#type: "color",
                                disabled,
                                value: rule.suffix_color.clone(),
                                oninput: {
                                    let mut form = form;
                                    move |event: FormEvent| {
                                        form.write().set_equipment_color_field(
                                            index,
                                            "suffix_color",
                                            event.value(),
                                        );
                                        on_dirty.call(());
                                    }
                                },
                            }
                        }
                        label { class: "admin-runtime-field",
                            span { {t!("admin-runtime-color-combined")} }
                            input {
                                r#type: "color",
                                disabled,
                                value: rule.combined_color.clone(),
                                oninput: {
                                    let mut form = form;
                                    move |event: FormEvent| {
                                        form.write().set_equipment_color_field(
                                            index,
                                            "combined_color",
                                            event.value(),
                                        );
                                        on_dirty.call(());
                                    }
                                },
                            }
                        }
                    }
                }
            }
            button {
                r#type: "button",
                class: "admin-btn-secondary",
                disabled: disabled || rules.len() >= 200,
                onclick: {
                    let mut form = form;
                    move |_| {
                        form.write().add_equipment_color();
                        on_dirty.call(());
                    }
                },
                {t!("admin-runtime-color-add")}
            }
        }
    }
}
