//! 表单模型与 JSON 文档之间的嵌套读写辅助。

use auction_contracts::EquipmentTaxonomyDto;
use serde_json::{Map, Value};

pub(super) fn nested_value<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(root, |value, key| value.as_object()?.get(key))
}

/// 旧 runtime 文档没有 `send_classes` 子表；首次切换开关时补建完整默认组，
/// 否则中间段缺失会让写入失败且开关无效果。逐键补全与服务端 `serde(default)`
/// 及保存校验的六键闭集一致。未编辑时不写回，保持文档指纹。
pub(super) fn ensure_send_classes_parent(document: &mut Value) {
    if nested_value(document, "operations.mail").is_some()
        && nested_value(document, "operations.mail.send_classes").is_none()
    {
        let defaults = [
            "account_reply",
            "intake_reply",
            "settlement_notice",
            "outbid_reminder",
            "player_summary",
            "operations_alert",
        ]
        .into_iter()
        .map(|name| (name.to_owned(), Value::Bool(true)))
        .collect();
        let _ = set_nested_value(document, "operations.mail.send_classes", Value::Object(defaults));
    }
}

pub(super) fn set_nested_value(root: &mut Value, path: &str, value: Value) -> Result<(), ()> {
    let mut segments = path.split('.').peekable();
    let mut current = root;
    while let Some(segment) = segments.next() {
        let object = current.as_object_mut().ok_or(())?;
        if segments.peek().is_none() {
            object.insert(segment.to_owned(), value);
            return Ok(());
        }
        current = object.get_mut(segment).ok_or(())?;
    }
    Err(())
}

pub(super) fn list_text(value: Option<&Value>) -> Result<String, ()> {
    value
        .and_then(Value::as_array)
        .ok_or(())?
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()
        .map(|values| values.join("\n"))
        .ok_or(())
}

/// 新数组和旧单值互斥；旧规则回填为一个已选项，双重非空输入失败关闭。
pub(super) fn color_selection_text(
    object: &Map<String, Value>,
    plural: &str,
    singular: &str,
) -> Result<String, ()> {
    let scalar = match object.get(singular) {
        Some(value) => value.as_str().ok_or(())?,
        None => "",
    };
    let selected = match object.get(plural) {
        Some(value) => list_text(Some(value))?,
        None => String::new(),
    };
    if !selected.is_empty() && !scalar.is_empty() {
        return Err(());
    }
    Ok(if selected.is_empty() { scalar.to_owned() } else { selected })
}

pub(super) fn legacy_material_selection(
    object: &Map<String, Value>,
    taxonomy: &EquipmentTaxonomyDto,
) -> Option<String> {
    let single = object.get("equipment_type")?.as_str()?;
    taxonomy
        .materials
        .iter()
        .any(|material| material.name.eq_ignore_ascii_case(single))
        .then(|| single.to_owned())
}

pub(super) fn lines(value: &str) -> Vec<Value> {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| Value::String(line.to_owned()))
        .collect()
}

#[cfg(test)]
mod send_class_switch_tests {
    use serde_json::json;

    use super::ensure_send_classes_parent;

    /// 旧文档（有 mail 组、无 send_classes）切换开关时必须能写入新子表。
    #[test]
    fn legacy_mail_group_gains_send_classes_on_first_toggle() {
        let mut document = json!({"operations": {"mail": {"managed": true}}});
        ensure_send_classes_parent(&mut document);
        assert_eq!(
            document["operations"]["mail"]["send_classes"],
            json!({
                "account_reply": true,
                "intake_reply": true,
                "settlement_notice": true,
                "outbid_reminder": true,
                "player_summary": true,
                "operations_alert": true
            }),
            "首次切换前应补建完整默认组"
        );
    }

    /// 未接管 mail 组时不创建子表，保持旧文档指纹。
    #[test]
    fn document_without_mail_group_is_untouched() {
        let mut document = json!({"operations": {}});
        ensure_send_classes_parent(&mut document);
        assert_eq!(document, json!({"operations": {}}));
    }
}
