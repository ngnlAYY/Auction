//! 论坛装备名称标色规则编辑器。

use super::*;

const EQUIPMENT_TYPES: &[&str] =
    &["One-Handed", "Two-Handed", "Staff", "Shield", "Cloth", "Light", "Heavy"];

const EQUIPMENT_KINDS: &[(&str, &str)] = &[
    // 只列出武器与盾牌种类；护甲部位不作为标色规则的装备种类。
    ("One-Handed", "Axe"),
    ("One-Handed", "Club"),
    ("One-Handed", "Rapier"),
    ("One-Handed", "Shortsword"),
    ("One-Handed", "Wakizashi"),
    ("Two-Handed", "Estoc"),
    ("Two-Handed", "Longsword"),
    ("Two-Handed", "Mace"),
    ("Two-Handed", "Katana"),
    ("Staff", "Oak Staff"),
    ("Staff", "Willow Staff"),
    ("Staff", "Redwood Staff"),
    ("Staff", "Katalox Staff"),
    ("Shield", "Buckler"),
    ("Shield", "Kite Shield"),
    ("Shield", "Force Shield"),
];

const WEAPON_PREFIXES: &[&str] =
    &["Ethereal", "Fiery", "Arctic", "Shocking", "Tempestuous", "Hallowed", "Demonic"];

const SHIELD_PREFIXES: &[&str] =
    &["Reinforced", "Agile", "Mithril", "Ruby", "Cobalt", "Amber", "Jade", "Zircon", "Onyx"];

const ARMOR_PREFIXES: &[&str] = &[
    "Charged",
    "Frugal",
    "Radiant",
    "Mystic",
    "Savage",
    "Shielding",
    "Reinforced",
    "Agile",
    "Mithril",
    "Ruby",
    "Cobalt",
    "Amber",
    "Jade",
    "Zircon",
    "Onyx",
];

const ALL_PREFIXES: &[&str] = &[
    "Ethereal",
    "Fiery",
    "Arctic",
    "Shocking",
    "Tempestuous",
    "Hallowed",
    "Demonic",
    "Reinforced",
    "Agile",
    "Mithril",
    "Ruby",
    "Cobalt",
    "Amber",
    "Jade",
    "Zircon",
    "Onyx",
    "Charged",
    "Frugal",
    "Radiant",
    "Mystic",
    "Savage",
    "Shielding",
];

const ONE_HANDED_SUFFIXES: &[&str] = &[
    "Slaughter",
    "Balance",
    "Nimble",
    "Swiftness",
    "Focus",
    "Vampire",
    "Illithid",
    "Banshee",
    "Ox",
    "Raccoon",
    "Cheetah",
    "Battlecaster",
];

const TWO_HANDED_SUFFIXES: &[&str] =
    &["Slaughter", "Balance", "Battlecaster", "Vampire", "Illithid", "Banshee", "Ox"];

const STAFF_SUFFIXES: &[&str] = &[
    "Destruction",
    "Focus",
    "Surtr",
    "Niflheim",
    "Mjolnir",
    "Freyr",
    "Heimdall",
    "Fenrir",
    "Elementalist",
    "Heaven-Sent",
    "Demon-Fiend",
    "Earth-walker",
    "Curse-weaver",
    "Fox",
    "Owl",
];

const SHIELD_SUFFIXES: &[&str] = &[
    "Barrier",
    "Nimble",
    "Battlecaster",
    "Fleet",
    "Protection",
    "Warding",
    "Dampening",
    "Deflection",
    "Stoneskin",
    "Turtle",
];

const ARMOR_SUFFIXES: &[&str] = &[
    "Protection",
    "Warding",
    "Fleet",
    "Battlecaster",
    "Elementalist",
    "Heaven-Sent",
    "Demon-Fiend",
    "Earth-walker",
    "Curse-weaver",
    "Raccoon",
    "Cheetah",
    "Fox",
    "Owl",
    "Surtr",
    "Niflheim",
    "Mjolnir",
    "Freyr",
    "Heimdall",
    "Fenrir",
    "Fire-eater",
    "Frost-born",
    "Thunder-child",
    "Wind-waker",
    "Thrice-blessed",
    "Spirit-ward",
    "Dampening",
    "Deflection",
    "Stoneskin",
];

const ALL_SUFFIXES: &[&str] = &[
    "Slaughter",
    "Balance",
    "Nimble",
    "Swiftness",
    "Focus",
    "Vampire",
    "Illithid",
    "Banshee",
    "Ox",
    "Raccoon",
    "Cheetah",
    "Battlecaster",
    "Destruction",
    "Surtr",
    "Niflheim",
    "Mjolnir",
    "Freyr",
    "Heimdall",
    "Fenrir",
    "Elementalist",
    "Heaven-Sent",
    "Demon-Fiend",
    "Earth-walker",
    "Curse-weaver",
    "Fox",
    "Owl",
    "Barrier",
    "Fleet",
    "Protection",
    "Warding",
    "Dampening",
    "Deflection",
    "Stoneskin",
    "Turtle",
    "Fire-eater",
    "Frost-born",
    "Thunder-child",
    "Wind-waker",
    "Thrice-blessed",
    "Spirit-ward",
];

fn prefixes_for(category: &str, equipment_kind: &str) -> &'static [&'static str] {
    match equipment_kind {
        "Buckler" | "Kite Shield" => &["Reinforced", "Agile", "Mithril"],
        "Force Shield" => &["Ruby", "Cobalt", "Amber", "Jade", "Zircon", "Onyx"],
        "Axe" | "Club" | "Rapier" | "Shortsword" | "Wakizashi" | "Estoc" | "Longsword" | "Mace"
        | "Katana" | "Oak Staff" | "Willow Staff" | "Redwood Staff" | "Katalox Staff" => {
            WEAPON_PREFIXES
        }
        _ => match category {
            "One-Handed" | "Two-Handed" | "Staff" => WEAPON_PREFIXES,
            "Shield" => SHIELD_PREFIXES,
            "Cloth" | "Light" | "Heavy" => ARMOR_PREFIXES,
            _ => ALL_PREFIXES,
        },
    }
}

fn suffixes_for(category: &str, equipment_kind: &str) -> &'static [&'static str] {
    match equipment_kind {
        "Axe" => &[
            "Slaughter",
            "Balance",
            "Battlecaster",
            "Vampire",
            "Illithid",
            "Banshee",
            "Ox",
            "Raccoon",
        ],
        "Club" => &[
            "Slaughter",
            "Balance",
            "Swiftness",
            "Nimble",
            "Battlecaster",
            "Focus",
            "Vampire",
            "Illithid",
            "Banshee",
            "Raccoon",
            "Cheetah",
        ],
        "Rapier" => &[
            "Slaughter",
            "Balance",
            "Nimble",
            "Battlecaster",
            "Vampire",
            "Illithid",
            "Banshee",
            "Raccoon",
            "Cheetah",
        ],
        "Shortsword" => &[
            "Slaughter",
            "Balance",
            "Swiftness",
            "Battlecaster",
            "Focus",
            "Vampire",
            "Illithid",
            "Banshee",
            "Ox",
            "Raccoon",
        ],
        "Wakizashi" => {
            &["Slaughter", "Balance", "Nimble", "Swiftness", "Battlecaster", "Raccoon", "Cheetah"]
        }
        "Estoc" | "Longsword" | "Mace" => {
            &["Slaughter", "Balance", "Battlecaster", "Vampire", "Illithid", "Banshee", "Ox"]
        }
        "Katana" => &["Slaughter", "Balance", "Battlecaster", "Ox"],
        "Oak Staff" => &["Destruction", "Focus", "Heimdall", "Earth-walker", "Fox", "Owl"],
        "Willow Staff" => &["Destruction", "Focus", "Fenrir", "Curse-weaver", "Fox", "Owl"],
        "Redwood Staff" => &[
            "Destruction",
            "Focus",
            "Surtr",
            "Niflheim",
            "Mjolnir",
            "Freyr",
            "Elementalist",
            "Fox",
            "Owl",
        ],
        "Katalox Staff" => &[
            "Destruction",
            "Focus",
            "Heimdall",
            "Fenrir",
            "Heaven-Sent",
            "Demon-Fiend",
            "Fox",
            "Owl",
        ],
        "Buckler" => {
            &["Barrier", "Nimble", "Battlecaster", "Fleet", "Protection", "Warding", "Turtle"]
        }
        "Kite Shield" => &[
            "Barrier",
            "Battlecaster",
            "Protection",
            "Warding",
            "Dampening",
            "Deflection",
            "Stoneskin",
            "Turtle",
        ],
        "Force Shield" => {
            &["Protection", "Warding", "Dampening", "Deflection", "Stoneskin", "Turtle"]
        }
        _ => match category {
            "One-Handed" => ONE_HANDED_SUFFIXES,
            "Two-Handed" => TWO_HANDED_SUFFIXES,
            "Staff" => STAFF_SUFFIXES,
            "Shield" => SHIELD_SUFFIXES,
            "Cloth" | "Light" | "Heavy" => ARMOR_SUFFIXES,
            _ => ALL_SUFFIXES,
        },
    }
}

fn retain_allowed(value: &str, allowed: &[&str]) -> String {
    selected_values(value)
        .into_iter()
        .filter(|value| allowed.iter().any(|candidate| candidate.eq_ignore_ascii_case(value)))
        .collect::<Vec<_>>()
        .join("\n")
}

const QUALITIES: &[&str] =
    &["Peerless", "Legendary", "Magnificent", "Exquisite", "Superior", "Average", "Fair", "Crude"];

fn equipment_kinds_for(category: &str) -> Vec<&'static str> {
    EQUIPMENT_KINDS
        .iter()
        .filter(|(candidate, _)| category.is_empty() || *candidate == category)
        .map(|(_, equipment_kind)| *equipment_kind)
        .collect()
}

fn selected_values(value: &str) -> Vec<String> {
    value.lines().map(str::trim).filter(|value| !value.is_empty()).map(str::to_owned).collect()
}

#[component]
pub(super) fn EquipmentColorRulesField(
    form: Signal<RuntimeConfigForm>,
    disabled: bool,
    on_dirty: EventHandler<()>,
) -> Element {
    let rules = form.read().equipment_colors();
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
                        label { class: "admin-runtime-field",
                            span { {t!("admin-runtime-color-category")} }
                            select {
                                disabled,
                                value: rule.category.clone(),
                                onchange: {
                                    let mut form = form;
                                    let current_kind = rule.equipment_type.clone();
                                    let current_prefixes = rule.prefixes.clone();
                                    let current_suffixes = rule.suffixes.clone();
                                    move |event: FormEvent| {
                                        let category = event.value();
                                        let kind_allowed = current_kind.is_empty()
                                            || equipment_kinds_for(&category)
                                                .contains(&current_kind.as_str());
                                        let kind = if kind_allowed {
                                            current_kind.clone()
                                        } else {
                                            String::new()
                                        };
                                        let prefixes = retain_allowed(
                                            &current_prefixes,
                                            prefixes_for(&category, &kind),
                                        );
                                        let suffixes = retain_allowed(
                                            &current_suffixes,
                                            suffixes_for(&category, &kind),
                                        );
                                        form.write().set_equipment_color_field(
                                            index,
                                            "category",
                                            category,
                                        );
                                        if !kind_allowed {
                                            form.write().set_equipment_color_field(
                                                index,
                                                "equipment_type",
                                                String::new(),
                                            );
                                        }
                                        form.write().set_equipment_color_field(
                                            index,
                                            "prefixes",
                                            prefixes,
                                        );
                                        form.write().set_equipment_color_field(
                                            index,
                                            "suffixes",
                                            suffixes,
                                        );
                                        on_dirty.call(());
                                    }
                                },
                                option { value: "", {t!("admin-runtime-color-any")} }
                                for category in EQUIPMENT_TYPES {
                                    option {
                                        value: "{category}",
                                        selected: rule.category == *category,
                                        "{category}"
                                    }
                                }
                            }
                        }
                        label { class: "admin-runtime-field",
                            span { {t!("admin-runtime-color-type")} }
                            select {
                                disabled,
                                value: rule.equipment_type.clone(),
                                onchange: {
                                    let mut form = form;
                                    let category = rule.category.clone();
                                    let current_prefixes = rule.prefixes.clone();
                                    let current_suffixes = rule.suffixes.clone();
                                    move |event: FormEvent| {
                                        let kind = event.value();
                                        let prefixes = retain_allowed(
                                            &current_prefixes,
                                            prefixes_for(&category, &kind),
                                        );
                                        let suffixes = retain_allowed(
                                            &current_suffixes,
                                            suffixes_for(&category, &kind),
                                        );
                                        form.write().set_equipment_color_field(
                                            index,
                                            "equipment_type",
                                            kind,
                                        );
                                        form.write().set_equipment_color_field(
                                            index,
                                            "prefixes",
                                            prefixes,
                                        );
                                        form.write().set_equipment_color_field(
                                            index,
                                            "suffixes",
                                            suffixes,
                                        );
                                        on_dirty.call(());
                                    }
                                },
                                option { value: "", {t!("admin-runtime-color-any")} }
                                for equipment_kind in equipment_kinds_for(&rule.category) {
                                    option {
                                        value: "{equipment_kind}",
                                        selected: rule.equipment_type == equipment_kind,
                                        "{equipment_kind}"
                                    }
                                }
                                if !rule.equipment_type.is_empty()
                                    && !equipment_kinds_for(&rule.category)
                                        .contains(&rule.equipment_type.as_str())
                                {
                                    option {
                                        value: "{rule.equipment_type}",
                                        selected: true,
                                        "{rule.equipment_type}"
                                    }
                                }
                            }
                        }
                        label { class: "admin-runtime-field",
                            span { {t!("admin-runtime-color-quality")} }
                            select {
                                disabled,
                                value: rule.quality.clone(),
                                onchange: {
                                    let mut form = form;
                                    move |event: FormEvent| {
                                        form.write().set_equipment_color_field(
                                            index,
                                            "quality",
                                            event.value(),
                                        );
                                        on_dirty.call(());
                                    }
                                },
                                option { value: "", {t!("admin-runtime-color-any")} }
                                for quality in QUALITIES {
                                    option {
                                        value: "{quality}",
                                        selected: rule.quality == *quality,
                                        "{quality}"
                                    }
                                }
                            }
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
                            options: prefixes_for(&rule.category, &rule.equipment_type).to_vec(),
                            value: rule.prefixes.clone(),
                            disabled,
                            on_dirty,
                        }
                        ColorMultiSelect {
                            form,
                            index,
                            field: "suffixes",
                            label: t!("admin-runtime-color-suffixes"),
                            options: suffixes_for(&rule.category, &rule.equipment_type).to_vec(),
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

/// 原生多选列表；选项变化后按行写回，保持现有文本列表契约。
#[component]
fn ColorMultiSelect(
    mut form: Signal<RuntimeConfigForm>,
    index: usize,
    field: &'static str,
    label: String,
    options: Vec<&'static str>,
    value: String,
    disabled: bool,
    on_dirty: EventHandler<()>,
) -> Element {
    let selected = selected_values(&value);
    let extras = selected
        .iter()
        .filter(|selected| {
            !options.iter().any(|option| option.eq_ignore_ascii_case(selected.as_str()))
        })
        .cloned()
        .collect::<Vec<_>>();
    let name = format!("color-{field}-{index}");
    rsx! {
        label { class: "admin-runtime-field admin-runtime-color-multi",
            span { "{label}" }
            select {
                name: name.clone(),
                multiple: true,
                size: "6",
                disabled,
                onchange: {
                    let name = name.clone();
                    move |event: FormEvent| {
                        let values = event
                            .get(&name)
                            .into_iter()
                            .filter_map(|value| match value {
                                FormValue::Text(text) => Some(text),
                                FormValue::File(_) => None,
                            })
                            .collect::<Vec<_>>();
                        form.write().set_equipment_color_field(
                            index,
                            field,
                            values.join("\n"),
                        );
                        on_dirty.call(());
                    }
                },
                for option in options {
                    option {
                        value: "{option}",
                        selected: selected.iter().any(|value| value.eq_ignore_ascii_case(option)),
                        "{option}"
                    }
                }
                for option in extras {
                    option { value: "{option}", selected: true, "{option}" }
                }
            }
            small { {t!("admin-runtime-color-multi-hint")} }
        }
    }
}
