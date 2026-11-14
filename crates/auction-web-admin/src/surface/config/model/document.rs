//! 运行配置表单到服务端完整闭集文档的序列化；空白或非法的中间态一律拒绝提交。

use serde_json::{Map, Number, Value};

use super::super::{early_bird, operational_fields};
use super::nested::{lines, nested_value, set_nested_value};
use super::{
    LIST_FIELDS, RuntimeConfigForm, SIGNED_NUMBER_FIELDS, TEXT_FIELDS, UNSIGNED_NUMBER_FIELDS,
};

impl RuntimeConfigForm {
    /// 生成服务端要求的完整闭集文档；任一数字处于空白或非法中间态时拒绝提交。
    pub(in crate::surface::config) fn to_document(&self) -> Result<Value, ()> {
        let mut document = self.document.clone();
        if self.boolean("operations.mail.managed")
            && nested_value(&document, "operations.mail.send_intervals").is_none()
            && self.inputs.iter().any(|(path, value)| {
                path.starts_with("operations.mail.send_intervals.")
                    && value.parse::<u64>().is_ok_and(|seconds| seconds != 0)
            })
        {
            set_nested_value(
                &mut document,
                "operations.mail.send_intervals",
                Value::Object(Map::new()),
            )?;
        }
        for path in TEXT_FIELDS.iter().chain(operational_fields::TEXT_FIELDS) {
            if !self.edits_field(path) {
                continue;
            }
            set_nested_value(
                &mut document,
                path,
                Value::String(self.required_input(path)?.to_owned()),
            )?;
        }
        for path in LIST_FIELDS.iter().chain(operational_fields::LIST_FIELDS) {
            if !self.edits_field(path) {
                continue;
            }
            let values = self
                .required_input(path)?
                .lines()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| Value::String(value.to_owned()))
                .collect();
            set_nested_value(&mut document, path, Value::Array(values))?;
        }
        for path in UNSIGNED_NUMBER_FIELDS.iter().chain(operational_fields::NUMBER_FIELDS) {
            if !self.edits_field(path) {
                continue;
            }
            let value = self.required_input(path)?.parse::<u64>().map_err(|_| ())?;
            if *path == "operations.mail.batch_timeout_seconds"
                && nested_value(&document, path).is_none()
                && value == 600
            {
                continue;
            }
            if matches!(
                *path,
                "operations.mail.account_reply_window_seconds"
                    | "operations.mail.explanation_mail_window_seconds"
            ) && nested_value(&document, path).is_none()
                && value == 86_400
            {
                continue;
            }
            if *path == "operations.mail.explanation_mail_limit"
                && nested_value(&document, path).is_none()
                && value == 10
            {
                continue;
            }
            if path.starts_with("operations.mail.send_intervals.")
                && nested_value(&document, "operations.mail.send_intervals").is_none()
                && value == 0
            {
                continue;
            }
            set_nested_value(&mut document, path, Value::Number(Number::from(value)))?;
        }
        for path in SIGNED_NUMBER_FIELDS {
            let value = self.required_input(path)?.parse::<i64>().map_err(|_| ())?;
            set_nested_value(&mut document, path, Value::Number(Number::from(value)))?;
        }
        if self.edits_field(early_bird::ROOT) {
            // 已有区块直接读取输入；只有旧文档缺省时才生成默认值判断是否需要补写。
            let changed = early_bird::block(&document).is_some()
                || early_bird::inputs(&Value::Null)?
                    .iter()
                    .any(|(path, value)| self.inputs.get(*path) != Some(value));
            if changed {
                let block = early_bird::ensure_block(&mut document)?;
                for path in early_bird::input_paths() {
                    let name = early_bird::field(path).ok_or(())?;
                    block.insert(
                        name.to_owned(),
                        early_bird::submitted_value(path, self.required_input(path)?)?,
                    );
                }
            }
        }
        let mailboxes = self
            .mailboxes
            .iter()
            .map(|mailbox| {
                let mut object = Map::new();
                object.insert("world_id".to_owned(), Value::String(mailbox.world_id.clone()));
                object.insert("mailbox_key".to_owned(), Value::String(mailbox.mailbox_key.clone()));
                Value::Object(object)
            })
            .collect();
        set_nested_value(&mut document, "dashboard_mailboxes", Value::Array(mailboxes))?;
        if self.had_equipment_colors || !self.equipment_colors.is_empty() {
            let rules = self
                .equipment_colors
                .iter()
                .map(|rule| {
                    let mut object = Map::new();
                    // 保存统一使用多选数组；清空兼容单值，避免同一维度存在两个解释。
                    for (plural, singular, values) in [
                        ("categories", "category", &rule.category),
                        ("equipment_types", "equipment_type", &rule.equipment_type),
                        ("qualities", "quality", &rule.quality),
                    ] {
                        if singular == "equipment_type"
                            && let Some(legacy) = &rule.legacy_equipment_type
                        {
                            // 旧单值材质原来只精确匹配，不能因编辑其他维度扩大成全族。
                            object.insert(plural.to_owned(), Value::Array(Vec::new()));
                            object.insert(singular.to_owned(), Value::String(legacy.clone()));
                        } else {
                            object.insert(plural.to_owned(), Value::Array(lines(values)));
                            object.insert(singular.to_owned(), Value::String(String::new()));
                        }
                    }
                    object.insert(
                        "level".to_owned(),
                        serde_json::json!({
                            "min": rule.level_min.parse::<i64>().map_err(|_| ())?,
                            "max": rule.level_max.parse::<i64>().map_err(|_| ())?,
                        }),
                    );
                    object.insert("prefixes".to_owned(), Value::Array(lines(&rule.prefixes)));
                    object.insert("suffixes".to_owned(), Value::Array(lines(&rule.suffixes)));
                    object.insert(
                        "prefix_color".to_owned(),
                        Value::String(rule.prefix_color.clone()),
                    );
                    object.insert(
                        "suffix_color".to_owned(),
                        Value::String(rule.suffix_color.clone()),
                    );
                    object.insert(
                        "combined_color".to_owned(),
                        Value::String(rule.combined_color.clone()),
                    );
                    Ok(Value::Object(object))
                })
                .collect::<Result<Vec<_>, ()>>()?;
            let mut colors = Map::new();
            colors.insert("rules".to_owned(), Value::Array(rules));
            set_nested_value(&mut document, "equipment_colors", Value::Object(colors))?;
        }
        Ok(document)
    }
}
