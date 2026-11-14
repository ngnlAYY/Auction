//! 只解码原生装备元数据的命名 JSON 字面量，不执行脚本或从其它对象猜测名称。

use std::collections::{HashMap, HashSet};
use std::fmt;

use auction_application::core::diagnostics::DiagnosticContext;
use scraper::{ElementRef, Html};
use serde::de::DeserializeSeed;
use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::{Deserializer, de};

use crate::clients::mooglemail::parser::{JsonText, executable_javascript, protocol, selector};
use crate::clients::mooglemail::{MoogleMailError, MoogleMailOperation, MoogleMailWorld};
use crate::safe_diagnostics;

#[derive(Default)]
pub(super) struct InventoryNames {
    pub(super) values: HashMap<String, String>,
    declared: [bool; 2],
}

pub(super) fn inventory_names(
    document: &Html,
    wanted: &HashSet<&str>,
    world: MoogleMailWorld,
) -> Result<(InventoryNames, Vec<String>), MoogleMailError> {
    let operation = MoogleMailOperation::Inventory;
    let fail = || protocol(operation, world);
    let mut names = InventoryNames::default();
    let mut sources = Vec::new();
    for script in document.select(&selector("script", operation, world)?) {
        let source = script.text().collect::<String>();
        if let Some(reference) = script.attr("src") {
            if source.contains("dynjs_equip") || source.contains("dynjs_eqstore") {
                return Err(fail());
            }
            if reference.contains("/dynjs/")
                || reference.contains("equip-")
                || reference.contains("eqstore-")
            {
                if sources.len() == 2
                    || sources.iter().any(|existing| existing == reference)
                    || !script_type(script)
                {
                    return Err(fail());
                }
                sources.push(reference.to_owned());
            }
            continue;
        }
        if source.contains("dynjs_equip") || source.contains("dynjs_eqstore") {
            if !script_type(script) {
                return Err(fail());
            }
            names.include(&source, wanted, world)?;
        }
    }
    Ok((names, sources))
}

fn script_type(script: ElementRef<'_>) -> bool {
    script
        .attr("type")
        .is_none_or(|value| matches!(value, "" | "text/javascript" | "application/javascript"))
}

impl InventoryNames {
    /// 外部脚本必须与当前库存页引用的装备/仓库种类一致，不能执行脚本或合并重复声明。
    pub(super) fn include_external(
        &mut self,
        reference: &str,
        bytes: &[u8],
        wanted: &HashSet<&str>,
        world: MoogleMailWorld,
    ) -> Result<(), MoogleMailError> {
        let fail = || protocol(MoogleMailOperation::Inventory, world);
        let expected =
            if reference.rsplit('/').next().is_some_and(|name| name.starts_with("equip-")) {
                0
            } else {
                1
            };
        let before = self.declared;
        self.include(
            str::from_utf8(bytes).map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_inventory",
                    "decode_name_script",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                fail()
            })?,
            wanted,
            world,
        )?;
        if before[expected]
            || !self.declared[expected]
            || self.declared[1 - expected] != before[1 - expected]
        {
            return Err(fail());
        }
        Ok(())
    }

    fn include(
        &mut self,
        source: &str,
        wanted: &HashSet<&str>,
        world: MoogleMailWorld,
    ) -> Result<(), MoogleMailError> {
        let fail = || protocol(MoogleMailOperation::Inventory, world);
        let masked = executable_javascript(source).ok_or_else(fail)?;
        for (index, variable) in ["dynjs_equip", "dynjs_eqstore"].into_iter().enumerate() {
            for (start, _) in masked.match_indices(variable) {
                let end = start + variable.len();
                if start > 0 && identifier(masked.as_bytes()[start - 1])
                    || masked.as_bytes().get(end).is_some_and(|byte| identifier(*byte))
                {
                    continue;
                }
                if self.declared[index] {
                    return Err(fail());
                }
                self.declared[index] = true;
                let json = object_assignment(source, &masked, start, end).ok_or_else(fail)?;
                let mut decoder = serde_json::Deserializer::from_str(json);
                EquipmentNames { wanted, names: &mut self.values }
                    .deserialize(&mut decoder)
                    .map_err(|diagnostic_cause| {
                        safe_diagnostics::warn(
                            "mooglemail_inventory",
                            "decode_name_map",
                            safe_diagnostics::project(&diagnostic_cause),
                            DiagnosticContext::default(),
                        );
                        fail()
                    })?;
                decoder.end().map_err(|diagnostic_cause| {
                    safe_diagnostics::warn(
                        "mooglemail_inventory",
                        "finish_name_map",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                    fail()
                })?;
            }
        }
        Ok(())
    }
}

fn identifier(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$')
}

/// 已观察的独立赋值可带 var/let/const；重复、属性改写、嵌套声明和表达式右值均拒绝。
fn object_assignment<'a>(
    source: &'a str,
    masked: &str,
    start: usize,
    end: usize,
) -> Option<&'a str> {
    let before = masked[..start].trim_end();
    let keyword_start =
        before.as_bytes().iter().rposition(|byte| !identifier(*byte)).map_or(0, |index| index + 1);
    let prefix = if matches!(&before[keyword_start..], "var" | "let" | "const") {
        before[..keyword_start].trim_end()
    } else {
        before
    };
    if !prefix.is_empty() && !prefix.ends_with(';') {
        return None;
    }
    let mut depth = [0_usize; 3];
    for byte in prefix.bytes() {
        match byte {
            b'{' => depth[0] = depth[0].checked_add(1)?,
            b'}' => depth[0] = depth[0].checked_sub(1)?,
            b'(' => depth[1] = depth[1].checked_add(1)?,
            b')' => depth[1] = depth[1].checked_sub(1)?,
            b'[' => depth[2] = depth[2].checked_add(1)?,
            b']' => depth[2] = depth[2].checked_sub(1)?,
            _ => {}
        }
    }
    if depth != [0; 3] {
        return None;
    }
    let after = masked[end..].trim_start().strip_prefix('=')?.trim_start();
    if !after.starts_with('{') {
        return None;
    }
    let object_start = masked.len() - after.len();
    let mut braces = 0_usize;
    for (offset, byte) in after.bytes().enumerate() {
        match byte {
            b'{' => braces = braces.checked_add(1)?,
            b'}' => {
                braces = braces.checked_sub(1)?;
                if braces == 0 {
                    let object_end = object_start + offset + 1;
                    if !masked[object_end..].trim_start().starts_with(';') {
                        return None;
                    }
                    return source.get(object_start..object_end);
                }
            }
            _ => {}
        }
    }
    None
}

struct EquipmentNames<'a, 'b> {
    wanted: &'a HashSet<&'b str>,
    names: &'a mut HashMap<String, String>,
}

impl<'de> DeserializeSeed<'de> for EquipmentNames<'_, '_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for EquipmentNames<'_, '_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("原生装备名称 JSON 对象")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut seen = HashSet::new();
        while let Some(JsonText(eid)) = map.next_key::<JsonText<'de>>()? {
            if seen.contains(eid.as_ref()) {
                return Err(de::Error::custom("重复装备元数据 EID"));
            }
            if self.wanted.contains(eid.as_ref()) {
                let DisplayName(name) = map.next_value::<DisplayName>()?;
                if self.names.insert(eid.to_string(), name).is_some() {
                    return Err(de::Error::custom("装备名称证据不唯一"));
                }
            } else {
                map.next_value::<IgnoredAny>()?;
            }
            seen.insert(eid);
        }
        Ok(())
    }
}

struct DisplayName(String);

impl<'de> serde::Deserialize<'de> for DisplayName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(DisplayNameVisitor)
    }
}

struct DisplayNameVisitor;

impl<'de> Visitor<'de> for DisplayNameVisitor {
    type Value = DisplayName;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("包含唯一 t 显示名的装备对象")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut name = None;
        let mut seen = HashSet::new();
        while let Some(JsonText(field)) = map.next_key::<JsonText<'de>>()? {
            if seen.contains(field.as_ref()) {
                return Err(de::Error::custom("重复装备元数据字段"));
            }
            if field == "t" {
                let value = map.next_value::<String>()?;
                if value.is_empty() || value.chars().any(char::is_control) {
                    return Err(de::Error::custom("装备显示名无效"));
                }
                name = Some(value);
            } else {
                map.next_value::<IgnoredAny>()?;
            }
            seen.insert(field);
        }
        name.map(DisplayName).ok_or_else(|| de::Error::custom("装备缺少显示名"))
    }
}
