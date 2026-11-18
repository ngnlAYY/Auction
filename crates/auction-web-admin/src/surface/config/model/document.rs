//! 运行配置表单到服务端完整闭集文档的序列化；空白或非法的中间态一律拒绝提交。

use serde_json::{Map, Number, Value};

use super::super::{early_bird, operational_fields};
use super::nested::{lines, list_text, set_nested_value};
use super::{
    LIST_FIELDS, MAX_DASHBOARD_MAILBOXES, RuntimeConfigForm, RuntimeEquipmentColorForm,
    SIGNED_NUMBER_FIELDS, TEXT_FIELDS, UNSIGNED_NUMBER_FIELDS,
};

impl RuntimeConfigForm {
    /// 生成服务端要求的完整闭集文档；任一数字处于空白或非法中间态时拒绝提交。
    pub(in crate::surface::config) fn to_document(&self) -> Result<Value, ()> {
        // 校验与写入都以读取时的投影为底。未编辑的值不经输入框往返，避免保存时改写兼容配置。
        let mut document = self.document.clone();
        for path in TEXT_FIELDS.iter().chain(operational_fields::TEXT_FIELDS) {
            if let Some(input) = self.edited_input(path) {
                set_nested_value(&mut document, path, Value::String(input.to_owned()))?;
            }
        }
        // 模式变化派生的清理仅作用于候选，撤销模式编辑时保留原值和用户显式的输入。
        let inactive_listener_fields: &[&str] = match self.edited_input("listener.mode") {
            Some("tcp") => {
                &["listener.group", "listener.runtime_directory", "listener.socket_path"]
            }
            Some("unix") => &["listener.address"],
            _ => &[],
        };
        for path in inactive_listener_fields {
            set_nested_value(&mut document, path, Value::String(String::new()))?;
        }
        for path in LIST_FIELDS.iter().chain(operational_fields::LIST_FIELDS) {
            if let Some(input) = self.edited_input(path) {
                set_nested_value(&mut document, path, Value::Array(lines(input)))?;
            }
        }
        for path in UNSIGNED_NUMBER_FIELDS.iter().chain(operational_fields::UNSIGNED_NUMBER_FIELDS)
        {
            if let Some(input) = self.edited_input(path) {
                let value = input.parse::<u64>().map_err(|_| ())?;
                set_nested_value(&mut document, path, Value::Number(Number::from(value)))?;
            }
        }
        for path in SIGNED_NUMBER_FIELDS.iter().chain(operational_fields::SIGNED_NUMBER_FIELDS) {
            if let Some(input) = self.edited_input(path) {
                let value = input.parse::<i64>().ok().ok_or(())?;
                set_nested_value(&mut document, path, Value::Number(Number::from(value)))?;
            }
        }
        for path in early_bird::input_paths() {
            if let Some(input) = self.edited_input(path) {
                let name = early_bird::field(path).ok_or(())?;
                early_bird::ensure_block(&mut document)?
                    .insert(name.to_owned(), early_bird::submitted_value(path, input)?);
            }
        }
        for (&path, &value) in &self.boolean_edits {
            if !self.edits_field(path) {
                continue;
            }
            if let Some(name) = early_bird::field(path) {
                early_bird::ensure_block(&mut document)?
                    .insert(name.to_owned(), Value::Bool(value));
            } else {
                set_nested_value(&mut document, path, Value::Bool(value))?;
            }
        }
        let mailboxes = self.mailboxes_document();
        if document.get("dashboard_mailboxes") != Some(&mailboxes) {
            set_nested_value(&mut document, "dashboard_mailboxes", mailboxes)?;
        }
        if self.had_equipment_colors || !self.equipment_colors.is_empty() {
            let source_rules =
                self.document.pointer("/equipment_colors/rules").and_then(Value::as_array);
            let rules = self
                .equipment_colors
                .iter()
                .map(|rule| {
                    let source = rule
                        .source_index
                        .map(|index| {
                            source_rules
                                .and_then(|rules| rules.get(index))
                                .and_then(Value::as_object)
                                .ok_or(())
                        })
                        .transpose()?;
                    equipment_color_document(rule, source)
                })
                .collect::<Result<Vec<_>, ()>>()?;
            if source_rules != Some(&rules) {
                set_nested_value(
                    &mut document,
                    "equipment_colors",
                    serde_json::json!({"rules": rules}),
                )?;
            }
        }
        Ok(document)
    }

    fn mailboxes_document(&self) -> Value {
        Value::Array(
            self.mailboxes
                .iter()
                .map(|mailbox| {
                    let mut object = Map::new();
                    object.insert("world_id".to_owned(), Value::String(mailbox.world_id.clone()));
                    object.insert(
                        "mailbox_key".to_owned(),
                        Value::String(mailbox.mailbox_key.clone()),
                    );
                    Value::Object(object)
                })
                .collect(),
        )
    }

    pub(in crate::surface::config) fn edited_mailboxes_exceed_limit(&self) -> bool {
        self.mailboxes.len() > MAX_DASHBOARD_MAILBOXES
            && self.document.get("dashboard_mailboxes") != Some(&self.mailboxes_document())
    }

    pub(in crate::surface::config) fn source_color_level(
        &self,
        index: usize,
        bound: &str,
    ) -> Option<i64> {
        let source_index = self.equipment_colors.get(index)?.source_index?;
        self.document
            .get("equipment_colors")?
            .get("rules")?
            .get(source_index)?
            .get("level")?
            .get(bound)?
            .as_i64()
    }
}

fn equipment_color_document(
    rule: &RuntimeEquipmentColorForm,
    source: Option<&Map<String, Value>>,
) -> Result<Value, ()> {
    let mut object = source.cloned().unwrap_or_default();
    for (plural, singular, selection) in [
        ("categories", "category", &rule.category),
        ("equipment_types", "equipment_type", &rule.equipment_type),
        ("qualities", "quality", &rule.quality),
    ] {
        selection.write(&mut object, source, plural, singular)?;
    }
    object.insert(
        "level".to_owned(),
        serde_json::json!({
            "min": rule.level_min.parse::<i64>().map_err(|_| ())?,
            "max": rule.level_max.parse::<i64>().map_err(|_| ())?,
        }),
    );
    for (name, value) in [("prefixes", &rule.prefixes), ("suffixes", &rule.suffixes)] {
        let original = source.map(|source| list_text(source.get(name))).transpose()?;
        if original.as_ref() != Some(value) {
            object.insert(name.to_owned(), Value::Array(lines(value)));
        }
    }
    for (name, value) in [
        ("prefix_color", &rule.prefix_color),
        ("suffix_color", &rule.suffix_color),
        ("combined_color", &rule.combined_color),
    ] {
        object.insert(name.to_owned(), Value::String(value.clone()));
    }
    Ok(Value::Object(object))
}

#[cfg(test)]
mod tests {
    use auction_contracts::{AdminRuntimeConfigResponse, ApiResponse};
    use serde_json::{Value, json};

    use super::super::nested::nested_value;
    use super::{
        LIST_FIELDS, RuntimeConfigForm, SIGNED_NUMBER_FIELDS, TEXT_FIELDS, UNSIGNED_NUMBER_FIELDS,
        early_bird, operational_fields,
    };

    fn fixture_document() -> Value {
        let mut document = json!({
            "metrics": { "enabled": false },
            "dashboard_mailboxes": [],
            "security": {
                "allowed_origins_enabled": false,
                "allowed_origins": ["", " https://example.invalid ", "\n", ""],
            },
            "operations": { "schedule": { "managed": false, "enabled": false } },
            "equipment_colors": { "rules": [{
                "category": "Cloth",
                "equipment_type": "Phase Robe",
                "quality": "Legendary",
                "level": { "min": 0, "max": 500 },
                "prefixes": ["Radiant"],
                "suffixes": [],
                "prefix_color": "#FF0000",
                "suffix_color": "#00aa00",
                "combined_color": "#aa00aa",
            }] },
        });
        for (paths, default) in [
            (
                TEXT_FIELDS
                    .iter()
                    .chain(operational_fields::TEXT_FIELDS)
                    .copied()
                    .collect::<Vec<_>>(),
                json!(""),
            ),
            (
                LIST_FIELDS
                    .iter()
                    .chain(operational_fields::LIST_FIELDS)
                    .copied()
                    .filter(|path| *path != "bot_players")
                    .collect(),
                json!([]),
            ),
            (
                UNSIGNED_NUMBER_FIELDS
                    .iter()
                    .chain(SIGNED_NUMBER_FIELDS)
                    .chain(operational_fields::SIGNED_NUMBER_FIELDS)
                    .chain(operational_fields::UNSIGNED_NUMBER_FIELDS)
                    .copied()
                    .collect(),
                json!(0),
            ),
        ] {
            for path in paths {
                let mut value = &mut document;
                for segment in path.split('.') {
                    value = &mut value[segment];
                }
                if value.is_null() {
                    *value = default.clone();
                }
            }
        }
        document
    }

    fn form(document: Value) -> RuntimeConfigForm {
        let response: AdminRuntimeConfigResponse = serde_json::from_value(json!({
            "document": document,
            "fingerprint": format!("sha256:{}", "0".repeat(64)),
            "restart_required": false,
            "equipment_taxonomy": {
                "revision": "test", "categories": [], "qualities": [], "kinds": [], "materials": [],
            },
        }))
        .unwrap();
        RuntimeConfigForm::from_response(&response).unwrap()
    }

    #[test]
    fn unrelated_edit_preserves_original_json_and_does_not_create_optional_fields() {
        let source = fixture_document();
        let mut form = form(source.clone());
        form.set_input("security.login_rate_limit", "10".to_owned());
        form.set_boolean("metrics.enabled", true);
        assert_eq!(form.source_document(), &source);
        let mut expected = source;
        expected["security"]["login_rate_limit"] = json!(10);
        expected["metrics"]["enabled"] = json!(true);
        assert_eq!(form.to_document().unwrap(), expected);
    }

    #[test]
    fn clearing_edited_origins_replaces_the_original_blank_entry() {
        let mut source = fixture_document();
        source["security"]["allowed_origins"] = json!([""]);
        let mut form = form(source.clone());
        assert!(form.input("security.allowed_origins").is_empty());
        assert_eq!(form.to_document().unwrap(), source);

        form.set_input("security.allowed_origins", "https://example.invalid".to_owned());
        form.set_input("security.allowed_origins", String::new());
        assert_eq!(form.to_document().unwrap()["security"]["allowed_origins"], json!([]));
    }

    #[test]
    fn listener_mode_round_trip_preserves_the_original_address_on_unrelated_edit() {
        let mut source = fixture_document();
        source["listener"]["mode"] = json!("tcp");
        source["listener"]["address"] = json!("[0:0:0:0:0:0:0:1]:8080");
        let mut form = form(source.clone());
        form.set_input("listener.mode", "unix".to_owned());
        assert_eq!(form.to_document().unwrap()["listener"]["address"], "");
        form.set_input("listener.mode", "tcp".to_owned());
        form.set_boolean("metrics.enabled", true);

        let mut expected = source;
        expected["metrics"]["enabled"] = json!(true);
        assert_eq!(form.to_document().unwrap(), expected);
        form.set_input("listener.address", "127.0.0.1:8080".to_owned());
        form.set_input("listener.mode", "unix".to_owned());
        form.set_input("listener.mode", "tcp".to_owned());
        assert_eq!(form.to_document().unwrap()["listener"]["address"], "127.0.0.1:8080");
    }

    #[test]
    fn restoring_unix_mode_preserves_the_original_socket_fields() {
        let mut source = fixture_document();
        source["listener"] = json!({
            "mode": "unix", "address": "", "group": "auction",
            "socket_path": "/run/hv-auction/admin.sock", "runtime_directory": "/run/hv-auction",
        });
        let mut form = form(source.clone());
        form.set_input("listener.mode", "tcp".to_owned());
        for field in ["group", "socket_path", "runtime_directory"] {
            assert_eq!(form.to_document().unwrap()["listener"][field], "");
        }
        form.set_input("listener.mode", "unix".to_owned());
        form.set_boolean("metrics.enabled", true);
        source["metrics"]["enabled"] = json!(true);
        assert_eq!(form.to_document().unwrap(), source);
    }

    #[test]
    fn editor_opens_and_preserves_values_pending_server_validation() {
        let body = include_str!(
            "../../../../../auction-contracts/tests/fixtures/admin/runtime-config-default.json"
        );
        let mut response: ApiResponse<AdminRuntimeConfigResponse> =
            serde_json::from_str(body).unwrap();
        let source = &mut response.data.document;
        source["security"]["trusted_proxy_cidrs"] = json!(["bad-cidr"]);
        source["equipment_source"]["allowed_path_prefixes"] = json!(["/equip/../"]);
        source["tracing"]["queue_capacity"] = json!(0);
        let mut form = RuntimeConfigForm::from_response(&response.data).unwrap();
        form.set_boolean("metrics.enabled", true);
        assert_eq!(form.edited_input("tracing.queue_capacity"), None);
        let mut expected = response.data.document;
        expected["metrics"]["enabled"] = json!(true);
        assert_eq!(form.to_document().unwrap(), expected);

        form.set_input("tracing.queue_capacity", "256".to_owned());
        assert_eq!(form.edited_input("tracing.queue_capacity"), Some("256"));
    }

    #[test]
    fn edited_arrays_and_color_fields_override_only_their_values() {
        let mut source = fixture_document();
        source["equipment_colors"]["rules"][0]["prefixes"] = json!([" Radiant ", ""]);
        let mut form = form(source.clone());
        form.set_input(
            "security.allowed_origins",
            " https://one.invalid\n\nhttps://two.invalid ".to_owned(),
        );
        form.set_equipment_color_field(0, "level_max", "600".to_owned());
        let mut expected = source;
        expected["security"]["allowed_origins"] =
            json!(["https://one.invalid", "https://two.invalid"]);
        expected["equipment_colors"]["rules"][0]["level"]["max"] = json!(600);
        assert_eq!(form.to_document().unwrap(), expected);
    }

    #[test]
    fn color_conflicts_change_representation_only_after_an_explicit_choice() {
        let mut source = fixture_document();
        let rule = &mut source["equipment_colors"]["rules"][0];
        rule["categories"] = json!(["Light"]);
        rule["equipment_types"] = json!(["Cotton"]);
        rule["qualities"] = json!(["Legendary"]);
        let mut form = form(source.clone());
        form.set_boolean("metrics.enabled", true);
        source["metrics"]["enabled"] = json!(true);
        assert_eq!(form.to_document().unwrap(), source);

        form.resolve_equipment_color_selection(0, "category", false);
        form.resolve_equipment_color_selection(0, "equipment_type", true);
        form.resolve_equipment_color_selection(0, "quality", false);
        let rule = source["equipment_colors"]["rules"][0].as_object_mut().unwrap();
        rule.insert("category".to_owned(), json!(""));
        rule.remove("equipment_types");
        rule.insert("quality".to_owned(), json!(""));
        assert_eq!(form.to_document().unwrap(), source);
    }

    #[test]
    fn excess_mailboxes_can_be_removed_without_rewriting_the_remaining_rows() {
        let mut source = fixture_document();
        source["dashboard_mailboxes"] = json!([
            {"world_id": "main", "mailbox_key": "main-box"},
            {"world_id": "isekai", "mailbox_key": "isekai-box"},
            {"world_id": "main", "mailbox_key": "extra-1"},
            {"world_id": "isekai", "mailbox_key": "extra-2"},
        ]);
        let mut form = form(source.clone());
        assert!(!form.edited_mailboxes_exceed_limit());
        assert_eq!(form.to_document().unwrap(), source);
        form.remove_mailbox(3);
        assert!(form.edited_mailboxes_exceed_limit());
        form.remove_mailbox(2);
        assert!(!form.edited_mailboxes_exceed_limit());
        source["dashboard_mailboxes"].as_array_mut().unwrap().truncate(2);
        assert_eq!(form.to_document().unwrap(), source);
    }

    #[test]
    fn restoring_a_legacy_material_keeps_its_original_spelling_and_exact_match() {
        let mut response: ApiResponse<AdminRuntimeConfigResponse> =
            serde_json::from_str(include_str!(
                "../../../../../auction-contracts/tests/fixtures/admin/runtime-config-default.json"
            ))
            .unwrap();
        let mut rule = fixture_document()["equipment_colors"]["rules"][0].clone();
        rule["equipment_type"] = json!("phase");
        response.data.document["equipment_colors"]["rules"] = json!([rule]);
        let mut form = RuntimeConfigForm::from_response(&response.data).unwrap();

        form.set_equipment_color_field(0, "equipment_type", "Cotton".to_owned());
        assert_eq!(form.equipment_colors()[0].legacy_equipment_type(), None);
        let updated = form.to_document().unwrap();
        let changed = &updated["equipment_colors"]["rules"][0];
        assert_eq!(changed["equipment_type"], "");
        assert_eq!(changed["equipment_types"], json!(["Cotton"]));

        form.set_equipment_color_field(0, "equipment_type", String::new());
        form.set_equipment_color_field(0, "equipment_type", "Phase".to_owned());
        assert_eq!(form.equipment_colors()[0].legacy_equipment_type().as_deref(), Some("phase"));
        assert_eq!(form.to_document().unwrap(), response.data.document);
    }

    #[test]
    fn turning_off_management_discards_all_nested_edits() {
        for initially_managed in [true, false] {
            let mut source = fixture_document();
            source["operations"]["schedule"]["managed"] = json!(initially_managed);
            let mut form = form(source.clone());
            form.set_boolean("operations.schedule.managed", true);
            form.set_boolean("operations.schedule.enabled", true);
            form.set_boolean("operations.schedule.early_bird.enabled", true);
            form.set_input("operations.schedule.preview_time", "18:00".to_owned());
            form.set_input("operations.schedule.worlds", "main".to_owned());
            form.set_input("operations.schedule.early_bird.live_minutes", String::new());
            assert!(form.to_document().is_err());
            form.set_boolean("operations.schedule.managed", false);
            let mut expected = source;
            expected["operations"]["schedule"]["managed"] = json!(false);
            assert_eq!(form.to_document().unwrap(), expected);
            form.set_boolean("operations.schedule.managed", true);
            expected["operations"]["schedule"]["managed"] = json!(true);
            assert_eq!(form.to_document().unwrap(), expected);
        }
    }

    #[test]
    fn managed_group_submits_its_edits_and_preserves_early_bird_defaults() {
        let mut form = form(fixture_document());
        form.set_boolean("operations.schedule.managed", true);
        form.set_boolean("operations.schedule.enabled", true);
        form.set_boolean("operations.schedule.early_bird.enabled", true);
        form.set_input("operations.schedule.early_bird.live_minutes", "360".to_owned());
        let document = form.to_document().unwrap();
        assert_eq!(nested_value(&document, "operations.schedule.enabled"), Some(&json!(true)));
        let bird = early_bird::block(&document).unwrap();
        assert_eq!(bird["enabled"], json!(true));
        assert_eq!(bird["live_minutes"], json!(360));
        assert_eq!(bird["session_days"], early_bird::default_block()["session_days"]);
    }
}
