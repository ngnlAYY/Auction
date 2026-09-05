//! Admin 手工装备录入表单。
//!
//! 旧版拍卖总览和 lot 草稿页都提供同一份录入能力。表单只负责收集来源与命令参数，
//! 具体解析、幂等和权限判断仍由既有 Admin API 与应用层负责。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use serde_json::Value;
use uuid::Uuid;

use crate::api::{AdminIntakeResult, add_admin_intake};
use crate::app::surface::validation::{FieldError, focus_field};
use crate::app::surface::{admin_api_error_message, run_admin_action};

/// 录入表单各字段的校验结果；None 表示该字段当前无错误。
/// 来源字段（URL 或 JSON）共用一个错误位，随来源类型切换到对应控件。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct IntakeFieldErrors {
    source: Option<IntakeFieldIssue>,
    price: Option<IntakeFieldIssue>,
    player: Option<IntakeFieldIssue>,
}

impl IntakeFieldErrors {
    /// 是否存在任一字段错误。
    const fn any(&self) -> bool {
        self.source.is_some() || self.price.is_some() || self.player.is_some()
    }

    /// 表单序上第一个错误字段的控件 id；来源字段按当前来源类型取 URL 或 JSON 控件。
    fn first_invalid_id(&self, source_kind: &str) -> Option<&'static str> {
        if self.source.is_some() {
            Some(if source_kind == "url" { "auction-intake-url" } else { "auction-intake-json" })
        } else if self.price.is_some() {
            Some("auction-intake-price")
        } else if self.player.is_some() {
            Some("auction-intake-player")
        } else {
            None
        }
    }
}

/// 字段错误的分类；渲染时翻译，保证切换语言后已展示的错误同步更新。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IntakeFieldIssue {
    Required,
    InvalidJson,
    NotPositiveInteger,
    /// 服务端返回卖家已被拉黑。
    PlayerBlacklisted,
    /// 服务端返回装备冲突。
    EquipmentConflict,
}

fn intake_field_issue_message(issue: IntakeFieldIssue) -> String {
    match issue {
        IntakeFieldIssue::Required => t!("admin-field-required"),
        IntakeFieldIssue::InvalidJson => t!("admin-intake-invalid-json"),
        IntakeFieldIssue::NotPositiveInteger => t!("admin-field-positive-integer"),
        IntakeFieldIssue::PlayerBlacklisted => t!("admin-error-auction-intake-player-blacklisted"),
        IntakeFieldIssue::EquipmentConflict => t!("admin-error-equipment-conflict"),
    }
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
    let mut result = use_signal(|| None::<AdminIntakeResult>);
    // 服务端返回的错误码；输入层校验错误由各字段信号承担，不再走整页提示。
    let mut failure = use_signal(|| None::<String>);
    let mut field_errors = use_signal(IntakeFieldErrors::default);
    let mut command_id_state = use_signal(|| None::<Uuid>);
    let empty_value = t!("value-empty");

    rsx! {
        section {
            class: "admin-auction-intake",
            aria_labelledby: "auction-lot-intake-title",
            aria_busy: busy().to_string(),
            h2 { id: "auction-lot-intake-title", {t!("admin-intake-title", world: world_id.clone())} }
            p { {t!("admin-intake-description")} }
            form {
                class: "admin-auction-intake-form",
                onsubmit: move |event| {
                    event.prevent_default();
                    if busy() {
                        return;
                    }
                    let source_kind_value = source_kind();
                    let equipment_url_value = equipment_url().trim().to_owned();
                    let equipment_json_value = equipment_json().trim().to_owned();
                    let player_id_value = player_id().trim().to_owned();
                    // 逐字段校验并定位错误；提交失败聚焦第一个错误字段。
                    let mut errors = IntakeFieldErrors::default();
                    if source_kind_value == "url" {
                        if equipment_url_value.is_empty() || equipment_url_value.len() > 2048 {
                            errors.source = Some(IntakeFieldIssue::Required);
                        }
                    } else if !valid_canonical_equipment_json(&equipment_json_value) {
                        errors.source = Some(IntakeFieldIssue::InvalidJson);
                    }
                    let start_price_value =
                        start_price().trim().parse::<i64>().ok().filter(|value| *value > 0);
                    if start_price_value.is_none() {
                        errors.price = Some(IntakeFieldIssue::NotPositiveInteger);
                    }
                    if player_id_value.is_empty() || player_id_value.len() > 256 {
                        errors.player = Some(IntakeFieldIssue::Required);
                    }
                    if errors.any() {
                        field_errors.set(errors);
                        if let Some(field_id) = errors.first_invalid_id(&source_kind_value) {
                            focus_field(field_id);
                        }
                        return;
                    }
                    // 起拍价校验未过时 errors 非空并在上面返回，这里必有值。
                    let Some(start_price) = start_price_value else {
                        return;
                    };
                    field_errors.set(IntakeFieldErrors::default());
                    failure.set(None);
                    let world_id = world_id.clone();
                    let command_id = command_id_state().unwrap_or_else(|| {
                        let value = Uuid::new_v4();
                        command_id_state.set(Some(value));
                        value
                    });
                    let mut result = result;
                    let mut failure = failure;
                    let mut field_errors = field_errors;
                    let mut reload_generation = reload_generation;
                    run_admin_action(
                        busy,
                        async move {
                            add_admin_intake(
                                command_id,
                                world_id,
                                source_kind_value,
                                equipment_url_value,
                                equipment_json_value,
                                start_price,
                                player_id_value,
                            )
                            .await
                        },
                        move |value| {
                            result.set(Some(value));
                            failure.set(None);
                            field_errors.set(IntakeFieldErrors::default());
                            command_id_state.set(None);
                            reload_generation.set(reload_generation().saturating_add(1));
                        },
                        move |code| {
                            result.set(None);
                            // 可归属字段的服务端错误落到字段旁，其余保持页面级反馈。
                            match code.as_str() {
                                "seller_blacklisted" | "auction_intake_player_blacklisted" => {
                                    field_errors.write().player =
                                        Some(IntakeFieldIssue::PlayerBlacklisted);
                                    focus_field("auction-intake-player");
                                }
                                "equipment_conflict" => {
                                    field_errors.write().source =
                                        Some(IntakeFieldIssue::EquipmentConflict);
                                    if let Some(field_id) =
                                        field_errors().first_invalid_id(&source_kind())
                                    {
                                        focus_field(field_id);
                                    }
                                }
                                _ => failure.set(Some(code)),
                            }
                        },
                    );
                },
                label {
                    r#for: "auction-intake-source",
                    {t!("admin-intake-source")}
                    select {
                        id: "auction-intake-source",
                        disabled: busy(),
                        value: source_kind(),
                        onchange: move |event| {
                            source_kind.set(event.value());
                            command_id_state.set(None);
                            result.set(None);
                            failure.set(None);
                            field_errors.write().source = None;
                        },
                        option { value: "url", {t!("admin-intake-source-url")} }
                        option { value: "json", {t!("admin-intake-source-json")} }
                    }
                }
                if source_kind() == "url" {
                    label {
                        r#for: "auction-intake-url",
                        {t!("admin-intake-equipment-url")}
                        input {
                            id: "auction-intake-url",
                            r#type: "url",
                            autocomplete: "off",
                            maxlength: "2048",
                            required: true,
                            disabled: busy(),
                            aria_invalid: field_errors().source.is_some().then_some("true"),
                            aria_describedby: field_errors().source.is_some().then_some("auction-intake-url-error"),
                            value: equipment_url(),
                            oninput: move |event| {
                                equipment_url.set(event.value());
                                command_id_state.set(None);
                                result.set(None);
                                failure.set(None);
                                field_errors.write().source = None;
                            },
                        }
                    }
                    FieldError {
                        id: "auction-intake-url-error",
                        message: field_errors().source.map(intake_field_issue_message),
                    }
                } else {
                    div { class: "admin-auction-intake-json",
                        label {
                            r#for: "auction-intake-json",
                            {t!("admin-intake-equipment-json")}
                        }
                        textarea {
                            id: "auction-intake-json",
                            aria_describedby: if field_errors().source.is_some() {
                                "auction-intake-json-hint auction-intake-json-error"
                            } else {
                                "auction-intake-json-hint"
                            },
                            aria_invalid: field_errors().source.is_some().then_some("true"),
                            rows: "8",
                            maxlength: "65536",
                            placeholder: t!("admin-intake-json-hint"),
                            required: true,
                            disabled: busy(),
                            value: equipment_json(),
                            oninput: move |event| {
                                equipment_json.set(event.value());
                                command_id_state.set(None);
                                result.set(None);
                                failure.set(None);
                                field_errors.write().source = None;
                            },
                        }
                        span { id: "auction-intake-json-hint", {t!("admin-intake-json-hint")} }
                        FieldError {
                            id: "auction-intake-json-error",
                            message: field_errors().source.map(intake_field_issue_message),
                        }
                    }
                }
                label {
                    r#for: "auction-intake-price",
                    {t!("admin-intake-start-price")}
                    input {
                        id: "auction-intake-price",
                        r#type: "number",
                        min: "1",
                        step: "1",
                        required: true,
                        disabled: busy(),
                        aria_invalid: field_errors().price.is_some().then_some("true"),
                        aria_describedby: field_errors().price.is_some().then_some("auction-intake-price-error"),
                        value: start_price(),
                        oninput: move |event| {
                            start_price.set(event.value());
                            command_id_state.set(None);
                            result.set(None);
                            failure.set(None);
                            field_errors.write().price = None;
                        },
                    }
                }
                FieldError {
                    id: "auction-intake-price-error",
                    message: field_errors().price.map(intake_field_issue_message),
                }
                label {
                    r#for: "auction-intake-player",
                    {t!("admin-intake-player-id")}
                    input {
                        id: "auction-intake-player",
                        r#type: "text",
                        autocomplete: "off",
                        maxlength: "256",
                        required: true,
                        disabled: busy(),
                        aria_invalid: field_errors().player.is_some().then_some("true"),
                        aria_describedby: field_errors().player.is_some().then_some("auction-intake-player-error"),
                        value: player_id(),
                        oninput: move |event| {
                            player_id.set(event.value());
                            command_id_state.set(None);
                            result.set(None);
                            failure.set(None);
                            field_errors.write().player = None;
                        },
                    }
                }
                FieldError {
                    id: "auction-intake-player-error",
                    message: field_errors().player.map(intake_field_issue_message),
                }
                button {
                    r#type: "submit",
                    disabled: busy(),
                    if busy() { {t!("admin-action-working")} } else { {t!("admin-intake-submit")} }
                }
            }
            if let Some(code) = failure() {
                div {
                    role: "alert",
                    aria_live: "assertive",
                    class: "admin-panel__note action-feedback action-feedback--error",
                    strong { {t!("admin-intake-error")} }
                    br {}
                    span { {admin_api_error_message(&code)} }
                }
            }
            if let Some(value) = result() {
                p { class: "admin-auction-intake-result action-feedback action-feedback--success", role: "status",
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
    if !level_valid || equipment.get("tradeable").and_then(Value::as_bool).is_none() {
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
