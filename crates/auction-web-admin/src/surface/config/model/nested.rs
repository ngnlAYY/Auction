//! 表单模型与 JSON 文档之间的嵌套读写辅助。

use auction_contracts::EquipmentTaxonomyDto;
use serde_json::{Map, Value};

pub(super) fn nested_value<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(root, |value, key| value.as_object()?.get(key))
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColorSelectionChoice {
    Single,
    Multiple,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::surface::config) struct ColorSelection {
    pub(in crate::surface::config) text: String,
    pub(in crate::surface::config) conflicting_single: Option<String>,
    resolved: Option<ColorSelectionChoice>,
}

impl ColorSelection {
    pub(super) fn from_source(
        object: &Map<String, Value>,
        plural: &str,
        singular: &str,
    ) -> Result<Self, ()> {
        let scalar = match object.get(singular) {
            Some(value) => value.as_str().ok_or(())?,
            None => "",
        };
        if let Some(value) = object.get(plural)
            && !value.as_array().ok_or(())?.is_empty()
        {
            return Ok(Self {
                text: list_text(Some(value))?,
                conflicting_single: (!scalar.is_empty()).then(|| scalar.to_owned()),
                resolved: None,
            });
        }
        Ok(Self { text: scalar.to_owned(), ..Self::default() })
    }

    pub(super) fn uses_legacy_syntax(&self) -> bool {
        self.conflicting_single.is_none() && self.resolved != Some(ColorSelectionChoice::Multiple)
    }

    pub(super) fn resolve(&mut self, keep_single: bool) {
        let Some(single) = self.conflicting_single.take() else { return };
        self.resolved = Some(if keep_single {
            self.text = single;
            ColorSelectionChoice::Single
        } else {
            ColorSelectionChoice::Multiple
        });
    }

    pub(super) fn write(
        &self,
        object: &mut Map<String, Value>,
        source: Option<&Map<String, Value>>,
        plural: &str,
        singular: &str,
    ) -> Result<(), ()> {
        // 原投影中的冲突需由管理员解决；保留单值时仍按精确匹配解释旧材质。
        if self.resolved == Some(ColorSelectionChoice::Single)
            && object.get(singular).and_then(Value::as_str) == Some(self.text.as_str())
        {
            object.remove(plural);
            return Ok(());
        }
        let original =
            source.map(|source| Self::from_source(source, plural, singular)).transpose()?;
        let same_values = original.as_ref().is_some_and(|original| original.text == self.text);
        if same_values && self.resolved.is_none() {
            return Ok(());
        }
        object.insert(singular.to_owned(), Value::String(String::new()));
        if !same_values {
            let selected = lines(&self.text);
            if selected.is_empty() {
                object.remove(plural);
            } else {
                object.insert(plural.to_owned(), Value::Array(selected));
            }
        }
        Ok(())
    }
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
