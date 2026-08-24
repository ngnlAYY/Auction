//! Admin 手工装备录入表单。
//!
//! 旧版拍卖总览和 lot 草稿页都提供同一份录入能力。表单只负责收集来源与命令参数，
//! 具体解析、幂等和权限判断仍由既有 Admin API 与应用层负责。

use dioxus::prelude::*;
use dioxus_i18n::t;
use serde_json::Value;
use uuid::Uuid;

use crate::api::{AdminIntakeResult, add_admin_intake};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntakeFailure {
    Invalid,
    InvalidJson,
    Request,
}

/// 展示手工装备录入表单，并在成功后触发所属页面重新加载。
#[component]
pub(super) fn AdminIntakeForm(world_id: String, reload_generation: Signal<u64>) -> Element {
    let mut source_kind = use_signal(|| "url".to_owned());
    let mut equipment_url = use_signal(String::new);
    let mut equipment_json = use_signal(String::new);
    let mut start_price = use_signal(String::new);
    let mut player_id = use_signal(String::new);
    let busy = use_signal(|| false);
    let result = use_signal(|| None::<AdminIntakeResult>);
    let mut failure = use_signal(|| None::<IntakeFailure>);
    let mut command_id_state = use_signal(|| None::<Uuid>);
    let empty_value = t!("value-empty");

    rsx! {
        section {
            class: "admin-panel admin-intake",
            aria_labelledby: "admin-intake-title",
            aria_busy: busy().to_string(),
            h2 { id: "admin-intake-title", {t!("admin-intake-title")} }
            p { class: "admin-panel__note", {t!("admin-intake-description")} }
            form {
                class: "admin-panel__actions admin-intake__form",
                onsubmit: move |event| {
                    event.prevent_default();
                    if busy() {
                        return;
                    }
                    let source_kind_value = source_kind();
                    let equipment_url_value = equipment_url().trim().to_owned();
                    let equipment_json_value = equipment_json().trim().to_owned();
                    let player_id_value = player_id().trim().to_owned();
                    let Some(start_price) = start_price().trim().parse::<i64>().ok().filter(|value| *value > 0) else {
                        failure.set(Some(IntakeFailure::Invalid));
                        return;
                    };
                    if player_id_value.is_empty() || player_id_value.len() > 256 {
                        failure.set(Some(IntakeFailure::Invalid));
                        return;
                    }
                    if source_kind_value == "url" {
                        if equipment_url_value.is_empty() || equipment_url_value.len() > 2048 {
                            failure.set(Some(IntakeFailure::Invalid));
                            return;
                        }
                    } else if !valid_canonical_equipment_json(&equipment_json_value) {
                        failure.set(Some(IntakeFailure::InvalidJson));
                        return;
                    }
                    failure.set(None);
                    let world_id = world_id.clone();
                    let command_id = command_id_state().unwrap_or_else(|| {
                        let value = Uuid::new_v4();
                        command_id_state.set(Some(value));
                        value
                    });
                    let mut busy = busy;
                    let mut result = result;
                    let mut failure = failure;
                    let mut reload_generation = reload_generation;
                    busy.set(true);
                    spawn(async move {
                        match add_admin_intake(
                            command_id,
                            world_id,
                            source_kind_value,
                            equipment_url_value,
                            equipment_json_value,
                            start_price,
                            player_id_value,
                        )
                        .await
                        {
                            Ok(value) => {
                                result.set(Some(value));
                                failure.set(None);
                                reload_generation.set(reload_generation().saturating_add(1));
                            }
                            Err(_) => {
                                result.set(None);
                                failure.set(Some(IntakeFailure::Request));
                            }
                        }
                        busy.set(false);
                    });
                },
                label {
                    r#for: "admin-intake-source",
                    {t!("admin-intake-source")}
                    select {
                        id: "admin-intake-source",
                        disabled: busy(),
                        value: source_kind(),
                        onchange: move |event| {
                            source_kind.set(event.value());
                            command_id_state.set(None);
                            failure.set(None);
                        },
                        option { value: "url", {t!("admin-intake-source-url")} }
                        option { value: "json", {t!("admin-intake-source-json")} }
                    }
                }
                if source_kind() == "url" {
                    label {
                        r#for: "admin-intake-equipment-url",
                        {t!("admin-intake-equipment-url")}
                        input {
                            id: "admin-intake-equipment-url",
                            r#type: "url",
                            autocomplete: "off",
                            maxlength: "2048",
                            required: true,
                            disabled: busy(),
                            value: equipment_url(),
                            oninput: move |event| {
                                equipment_url.set(event.value());
                                command_id_state.set(None);
                                failure.set(None);
                            },
                        }
                    }
                } else {
                    label { class: "admin-panel__wide-field admin-intake__json",
                        r#for: "admin-intake-equipment-json",
                        {t!("admin-intake-equipment-json")}
                        textarea {
                            id: "admin-intake-equipment-json",
                            aria_describedby: "admin-intake-json-hint",
                            rows: "8",
                            maxlength: "65536",
                            placeholder: t!("admin-intake-json-hint"),
                            required: true,
                            disabled: busy(),
                            value: equipment_json(),
                            oninput: move |event| {
                                equipment_json.set(event.value());
                                command_id_state.set(None);
                                failure.set(None);
                            },
                        }
                        span { id: "admin-intake-json-hint", class: "admin-panel__note", {t!("admin-intake-json-hint")} }
                    }
                }
                label {
                    r#for: "admin-intake-start-price",
                    {t!("admin-intake-start-price")}
                    input {
                        id: "admin-intake-start-price",
                        r#type: "number",
                        min: "1",
                        step: "1",
                        required: true,
                        disabled: busy(),
                        value: start_price(),
                        oninput: move |event| {
                            start_price.set(event.value());
                            command_id_state.set(None);
                            failure.set(None);
                        },
                    }
                }
                label {
                    r#for: "admin-intake-player-id",
                    {t!("admin-intake-player-id")}
                    input {
                        id: "admin-intake-player-id",
                        r#type: "text",
                        autocomplete: "off",
                        maxlength: "256",
                        required: true,
                        disabled: busy(),
                        value: player_id(),
                        oninput: move |event| {
                            player_id.set(event.value());
                            command_id_state.set(None);
                            failure.set(None);
                        },
                    }
                }
                button {
                    r#type: "submit",
                    disabled: busy(),
                    if busy() { {t!("admin-action-working")} } else { {t!("admin-intake-submit")} }
                }
            }
            if let Some(current_failure) = failure() {
                p {
                    role: "alert",
                    class: "admin-panel__note action-feedback action-feedback--error",
                    {intake_failure_message(current_failure)}
                }
            }
            if let Some(value) = result() {
                p { class: "admin-panel__note action-feedback action-feedback--success", role: "status",
                    {t!(
                        "admin-intake-result",
                        name: value.equipment_name,
                        number: value.auction_sequence,
                        lot: value.lot_number,
                        category: value.equipment_category,
                        level: value
                            .equipment_level
                            .map(|level| level.to_string())
                            .unwrap_or_else(|| empty_value.clone())
                    )}
                }
            }
        }
    }
}

fn intake_failure_message(failure: IntakeFailure) -> String {
    match failure {
        IntakeFailure::Invalid => t!("admin-intake-invalid"),
        IntakeFailure::InvalidJson => t!("admin-intake-invalid-json"),
        IntakeFailure::Request => t!("admin-intake-error"),
    }
}

/// 在请求发送前复现旧版对 canonical JSON 的结构校验，避免把明显错误的 JSON 交给网络层。
fn valid_canonical_equipment_json(value: &str) -> bool {
    let Ok(Value::Object(equipment)) = serde_json::from_str::<Value>(value) else {
        return false;
    };
    const EQUIPMENT_KEYS: [&str; 11] = [
        "external_equipment_id",
        "name",
        "category",
        "equipment_type",
        "quality",
        "prefix",
        "suffix",
        "level",
        "tradeable",
        "stats",
        "key",
    ];
    if equipment.len() != EQUIPMENT_KEYS.len()
        || EQUIPMENT_KEYS.iter().any(|key| !equipment.contains_key(*key))
    {
        return false;
    }
    let Some(external_equipment_id) =
        equipment.get("external_equipment_id").and_then(Value::as_str)
    else {
        return false;
    };
    if external_equipment_id.is_empty()
        || external_equipment_id.len() > 64
        || !external_equipment_id.bytes().all(|byte| byte.is_ascii_digit())
    {
        return false;
    }
    for (key, maximum) in [("name", 512_usize), ("category", 128), ("equipment_type", 128)] {
        if !canonical_text(equipment.get(key), maximum) {
            return false;
        }
    }
    for key in ["quality", "prefix", "suffix"] {
        if !canonical_optional_text(equipment.get(key), 128) {
            return false;
        }
    }
    let level_valid = equipment.get("level").is_some_and(|level| {
        level.is_null() || level.as_i64().is_some_and(|value| (1..=100_000).contains(&value))
    });
    if !level_valid || equipment.get("tradeable") != Some(&Value::Bool(true)) {
        return false;
    }
    let Some(Value::Object(stats)) = equipment.get("stats") else {
        return false;
    };
    if stats.is_empty() || stats.len() > 256 {
        return false;
    }
    if !stats.iter().all(|(name, stat)| valid_canonical_stat(name, stat)) {
        return false;
    }
    equipment
        .get("key")
        .and_then(Value::as_str)
        .is_some_and(|key| key.len() == 10 && key.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn canonical_text(value: Option<&Value>, maximum: usize) -> bool {
    value.and_then(Value::as_str).is_some_and(|value| {
        !value.is_empty()
            && value.len() <= maximum
            && value == value.trim()
            && !value.chars().any(char::is_control)
    })
}

fn canonical_optional_text(value: Option<&Value>, maximum: usize) -> bool {
    value.and_then(Value::as_str).is_some_and(|value| {
        value.len() <= maximum && value == value.trim() && !value.chars().any(char::is_control)
    })
}

fn valid_canonical_stat(name: &str, value: &Value) -> bool {
    if name.is_empty()
        || name.len() > 128
        || name != name.trim()
        || name.chars().any(char::is_control)
    {
        return false;
    }
    let Value::Object(stat) = value else {
        return false;
    };
    if stat.keys().any(|key| key != "value" && key != "base") {
        return false;
    }
    if !canonical_text(stat.get("value"), 256) {
        return false;
    }
    match stat.get("base") {
        None | Some(Value::Null) => true,
        Some(value) => value
            .as_f64()
            .is_some_and(|value| value.is_finite() && (0.0..=1_000_000.0).contains(&value)),
    }
}
