//! 运行时配置页面的安全表单状态与完整 JSON 投影转换。

use std::{collections::BTreeMap, rc::Rc};

use auction_contracts::{AdminRuntimeConfigResponse, EquipmentTaxonomyDto};
use serde_json::Value;

use super::{early_bird, operational_fields};

mod document;
mod nested;

use self::nested::{ColorSelection, legacy_material_selection, list_text, nested_value};

const TEXT_FIELDS: &[&str] = &[
    "listener.address",
    "listener.group",
    "listener.mode",
    "listener.runtime_directory",
    "listener.socket_path",
    "tracing.endpoint",
    "worker_control.socket_path",
];

const LIST_FIELDS: &[&str] = &[
    "bot_players",
    "worlds.main.sender_allowlist",
    "worlds.isekai.sender_allowlist",
    "equipment_filter.qualities",
    "equipment_filter.prefixes",
    "equipment_filter.suffixes",
    "equipment_filter.categories",
    "equipment_filter.equipment_types",
    "equipment_source.allowed_hosts",
    "equipment_source.allowed_path_prefixes",
    "security.allowed_origins",
    "security.trusted_proxy_cidrs",
];

const UNSIGNED_NUMBER_FIELDS: &[&str] = &[
    "equipment_source.maximum_response_bytes",
    "equipment_source.minimum_request_interval_millis",
    "equipment_source.request_timeout_seconds",
    "security.login_rate_limit",
    "security.mutation_rate_limit",
    "security.rate_limit_window_seconds",
    "security.request_timeout_seconds",
    "session.idle_ttl_seconds",
    "session.absolute_ttl_seconds",
    "session.max_public_sessions",
    "tracing.batch_timeout_seconds",
    "tracing.export_timeout_seconds",
    "tracing.max_export_batch_size",
    "tracing.queue_capacity",
    "tracing.sample_ratio_ppm",
    "tracing.shutdown_timeout_seconds",
];

const SIGNED_NUMBER_FIELDS: &[&str] = &["equipment_filter.level.min", "equipment_filter.level.max"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct RuntimeMailboxForm {
    pub(super) world_id: String,
    pub(super) mailbox_key: String,
}

pub(super) const MAX_DASHBOARD_MAILBOXES: usize = 2;

/// 论坛装备标色规则的前端编辑状态；多选列表按行保存，兼容旧单值规则回填。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct RuntimeEquipmentColorForm {
    source_index: Option<usize>,
    pub(super) row_id: usize,
    pub(super) category: ColorSelection,
    pub(super) equipment_type: ColorSelection,
    /// 旧材质单值保持精确匹配，最终选项变更时才改用多选。
    legacy_equipment_type: Option<String>,
    pub(super) quality: ColorSelection,
    pub(super) level_min: String,
    pub(super) level_max: String,
    pub(super) prefixes: String,
    pub(super) suffixes: String,
    pub(super) prefix_color: String,
    pub(super) suffix_color: String,
    pub(super) combined_color: String,
}

impl RuntimeEquipmentColorForm {
    pub(super) fn legacy_equipment_type(&self) -> Option<String> {
        self.legacy_equipment_type
            .as_ref()
            .filter(|original| {
                self.equipment_type.uses_legacy_syntax() && *original == &self.equipment_type.text
            })
            .cloned()
    }

    fn selection_mut(&mut self, field: &str) -> Option<&mut ColorSelection> {
        match field {
            "category" => Some(&mut self.category),
            "equipment_type" => Some(&mut self.equipment_type),
            "quality" => Some(&mut self.quality),
            _ => None,
        }
    }
}

/// 页面保留服务端的完整安全投影，并只把可编辑标量转成适合浏览器输入的文本。
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct RuntimeConfigForm {
    document: Value,
    inputs: BTreeMap<&'static str, String>,
    input_edits: BTreeMap<&'static str, String>,
    boolean_edits: BTreeMap<&'static str, bool>,
    mailboxes: Vec<RuntimeMailboxForm>,
    equipment_colors: Vec<RuntimeEquipmentColorForm>,
    next_color_row_id: usize,
    had_equipment_colors: bool,
    equipment_taxonomy: Option<Rc<EquipmentTaxonomyDto>>,
}

impl RuntimeConfigForm {
    /// 读取编辑开始时的完整非敏感投影，供变更清单比对；不参与服务端授权。
    pub(super) fn source_document(&self) -> &Value {
        &self.document
    }

    /// 词典由同次响应单独下发，既不并入业务文档，也不参与其 fingerprint。
    pub(super) fn from_response(response: &AdminRuntimeConfigResponse) -> Result<Self, ()> {
        let taxonomy = response.equipment_taxonomy.as_ref().ok_or(())?;
        let document = &response.document;
        let mut inputs = BTreeMap::new();
        for path in TEXT_FIELDS.iter().chain(operational_fields::TEXT_FIELDS) {
            let value = nested_value(document, path).and_then(Value::as_str).ok_or(())?;
            inputs.insert(*path, value.to_owned());
        }
        for path in LIST_FIELDS.iter().chain(operational_fields::LIST_FIELDS) {
            // bot_players 对旧文档缺省时按空列表展示，其余列表仍必须是显式数组。
            let text = match nested_value(document, path) {
                Some(values) => values
                    .as_array()
                    .ok_or(())?
                    .iter()
                    .map(Value::as_str)
                    .collect::<Option<Vec<_>>>()
                    .ok_or(())?
                    .join("\n"),
                None if *path == "bot_players" => String::new(),
                None => return Err(()),
            };
            inputs.insert(*path, text);
        }
        // 服务端投影已补齐邮件区块的默认值；任何数字缺失都是响应损坏，不在浏览器补默认值。
        for path in UNSIGNED_NUMBER_FIELDS.iter().chain(operational_fields::UNSIGNED_NUMBER_FIELDS)
        {
            let value = nested_value(document, path).and_then(Value::as_u64).ok_or(())?;
            inputs.insert(*path, value.to_string());
        }
        for path in SIGNED_NUMBER_FIELDS.iter().chain(operational_fields::SIGNED_NUMBER_FIELDS) {
            let value = nested_value(document, path).and_then(Value::as_i64).ok_or(())?;
            inputs.insert(*path, value.to_string());
        }
        // 旧文档缺省早鸟区块时按默认值展示；未改动不会写回，保持原指纹。
        for (path, value) in early_bird::inputs(document)? {
            inputs.insert(path, value);
        }
        let mailboxes = nested_value(document, "dashboard_mailboxes")
            .and_then(Value::as_array)
            .ok_or(())?
            .iter()
            .map(|mailbox| {
                let object = mailbox.as_object().ok_or(())?;
                Ok(RuntimeMailboxForm {
                    world_id: object.get("world_id").and_then(Value::as_str).ok_or(())?.to_owned(),
                    mailbox_key: object
                        .get("mailbox_key")
                        .and_then(Value::as_str)
                        .ok_or(())?
                        .to_owned(),
                })
            })
            .collect::<Result<Vec<_>, ()>>()?;
        let had_equipment_colors = nested_value(document, "equipment_colors").is_some();
        let equipment_colors = nested_value(document, "equipment_colors")
            .and_then(|value| value.get("rules"))
            .and_then(Value::as_array)
            .map(|rules| {
                rules
                    .iter()
                    .enumerate()
                    .map(|(index, rule)| {
                        let object = rule.as_object().ok_or(())?;
                        Ok(RuntimeEquipmentColorForm {
                            source_index: Some(index),
                            row_id: index,
                            category: ColorSelection::from_source(
                                object,
                                "categories",
                                "category",
                            )?,
                            equipment_type: ColorSelection::from_source(
                                object,
                                "equipment_types",
                                "equipment_type",
                            )?,
                            legacy_equipment_type: legacy_material_selection(object, taxonomy),
                            quality: ColorSelection::from_source(object, "qualities", "quality")?,
                            level_min: object
                                .get("level")
                                .and_then(|level| level.get("min"))
                                .and_then(Value::as_i64)
                                .unwrap_or(0)
                                .to_string(),
                            level_max: object
                                .get("level")
                                .and_then(|level| level.get("max"))
                                .and_then(Value::as_i64)
                                .unwrap_or(0)
                                .to_string(),
                            prefixes: list_text(object.get("prefixes"))?,
                            suffixes: list_text(object.get("suffixes"))?,
                            prefix_color: object
                                .get("prefix_color")
                                .and_then(Value::as_str)
                                .ok_or(())?
                                .to_owned(),
                            suffix_color: object
                                .get("suffix_color")
                                .and_then(Value::as_str)
                                .ok_or(())?
                                .to_owned(),
                            combined_color: object
                                .get("combined_color")
                                .and_then(Value::as_str)
                                .ok_or(())?
                                .to_owned(),
                        })
                    })
                    .collect::<Result<Vec<_>, ()>>()
            })
            .transpose()?
            .unwrap_or_default();
        Ok(Self {
            document: document.clone(),
            inputs,
            input_edits: BTreeMap::new(),
            boolean_edits: BTreeMap::new(),
            mailboxes,
            next_color_row_id: equipment_colors.len(),
            equipment_colors,
            had_equipment_colors,
            equipment_taxonomy: Some(Rc::new(taxonomy.clone())),
        })
    }

    pub(super) fn equipment_taxonomy(&self) -> Option<Rc<EquipmentTaxonomyDto>> {
        self.equipment_taxonomy.clone()
    }

    /// 刷新候选元数据不改草稿或旧单值标记；历史值始终保留到用户明确编辑。
    pub(super) fn refresh_equipment_taxonomy(&mut self, taxonomy: &EquipmentTaxonomyDto) {
        let mut refreshed = taxonomy.clone();
        if let Some(previous) = self.equipment_taxonomy.as_deref()
            && previous.revision == refreshed.revision
        {
            // GET 只描述服务端文档；冲突草稿仍可能引用服务端已移除的有限种类。
            // 同版本仅保留草稿仍选中的既有权威候选，新响应同名优先，不在浏览器推断语法。
            for kind in &previous.resolved_kinds {
                let selected = self.equipment_colors.iter().any(|rule| {
                    rule.equipment_type
                        .text
                        .lines()
                        .any(|value| value.eq_ignore_ascii_case(&kind.name))
                });
                if selected
                    && !refreshed
                        .resolved_kinds
                        .iter()
                        .any(|current| current.name.eq_ignore_ascii_case(&kind.name))
                {
                    refreshed.resolved_kinds.push(kind.clone());
                }
            }
        }
        if self.equipment_taxonomy.as_deref() != Some(&refreshed) {
            self.equipment_taxonomy = Some(Rc::new(refreshed));
        }
    }

    /// 服务端词典独立演进不构成待保存修改，非法数字等原始编辑中间态仍参与比较。
    pub(super) fn same_edits_as(&self, other: &Self) -> bool {
        self.document == other.document
            && self.input_edits == other.input_edits
            && self.boolean_edits == other.boolean_edits
            && self.mailboxes == other.mailboxes
            && self.equipment_colors == other.equipment_colors
            && self.had_equipment_colors == other.had_equipment_colors
    }

    pub(super) fn input(&self, path: &str) -> String {
        self.input_edits.get(path).or_else(|| self.inputs.get(path)).cloned().unwrap_or_default()
    }

    pub(super) fn source_input(&self, path: &str) -> Option<&str> {
        self.inputs.get(path).map(String::as_str)
    }

    pub(super) fn set_input(&mut self, path: &'static str, value: String) {
        if !self.edits_field(path) {
            return;
        }
        // 非空原始数组也可能回填为空文本。只有用户编辑后清空才写入空数组。
        let clears_origins = path == "security.allowed_origins"
            && value.is_empty()
            && nested_value(&self.document, path)
                .and_then(Value::as_array)
                .is_some_and(|origins| !origins.is_empty());
        if !clears_origins && self.inputs.get(path) == Some(&value) {
            self.input_edits.remove(path);
        } else {
            self.input_edits.insert(path, value);
        }
    }

    pub(super) fn boolean(&self, path: &str) -> bool {
        self.boolean_edits.get(path).copied().unwrap_or_else(|| self.source_boolean(path))
    }

    fn source_boolean(&self, path: &str) -> bool {
        if let Some(value) = early_bird::boolean(&self.document, path) {
            return value;
        }
        nested_value(&self.document, path).and_then(Value::as_bool).unwrap_or(false)
    }

    pub(super) fn set_boolean(&mut self, path: &'static str, value: bool) {
        if !self.edits_field(path) {
            return;
        }
        // 关闭接管即丢弃组内未保存的输入，之后重新接管也从原始值开始。
        if !value && operational_fields::management_path(path) == Some(path) {
            self.input_edits
                .retain(|field, _| operational_fields::management_path(field) != Some(path));
            self.boolean_edits
                .retain(|field, _| operational_fields::management_path(field) != Some(path));
        }
        if value == self.source_boolean(path) {
            self.boolean_edits.remove(path);
        } else {
            self.boolean_edits.insert(path, value);
        }
    }

    pub(super) fn mailboxes(&self) -> Vec<RuntimeMailboxForm> {
        self.mailboxes.clone()
    }

    pub(super) fn add_mailbox(&mut self) {
        if self.mailboxes.len() >= MAX_DASHBOARD_MAILBOXES {
            return;
        }
        let world_id = ["main", "isekai"]
            .into_iter()
            .find(|world| self.mailboxes.iter().all(|mailbox| mailbox.world_id != *world))
            .unwrap_or("main")
            .to_owned();
        let mailbox_key = format!("mooglemail:{world_id}");
        self.mailboxes.push(RuntimeMailboxForm { world_id, mailbox_key });
    }

    pub(super) fn remove_mailbox(&mut self, index: usize) {
        if index < self.mailboxes.len() {
            self.mailboxes.remove(index);
        }
    }

    pub(super) fn set_mailbox_world(&mut self, index: usize, value: String) {
        if let Some(mailbox) = self.mailboxes.get_mut(index) {
            mailbox.world_id = value;
        }
    }

    pub(super) fn set_mailbox_key(&mut self, index: usize, value: String) {
        if let Some(mailbox) = self.mailboxes.get_mut(index) {
            mailbox.mailbox_key = value;
        }
    }

    /// 展示与单项编辑只借用集合，避免每次勾选复制全部规则及其文本。
    pub(super) fn equipment_colors(&self) -> &[RuntimeEquipmentColorForm] {
        &self.equipment_colors
    }

    pub(super) fn add_equipment_color(&mut self) {
        self.equipment_colors.push(RuntimeEquipmentColorForm {
            source_index: None,
            row_id: self.next_color_row_id,
            category: ColorSelection::default(),
            equipment_type: ColorSelection::default(),
            legacy_equipment_type: None,
            quality: ColorSelection::default(),
            level_min: "0".to_owned(),
            level_max: "0".to_owned(),
            prefixes: String::new(),
            suffixes: String::new(),
            prefix_color: "#ff0000".to_owned(),
            suffix_color: "#00aa00".to_owned(),
            combined_color: "#aa00aa".to_owned(),
        });
        self.next_color_row_id += 1;
    }

    pub(super) fn remove_equipment_color(&mut self, index: usize) {
        if index < self.equipment_colors.len() {
            self.equipment_colors.remove(index);
        }
    }

    pub(super) fn move_equipment_color(&mut self, index: usize, delta: isize) {
        let Some(target) = index.checked_add_signed(delta) else { return };
        if index < self.equipment_colors.len() && target < self.equipment_colors.len() {
            self.equipment_colors.swap(index, target);
        }
    }

    pub(super) fn set_equipment_color_field(
        &mut self,
        index: usize,
        field: &str,
        mut value: String,
    ) {
        let Some(rule) = self.equipment_colors.get_mut(index) else { return };
        if field == "equipment_type"
            && rule.equipment_type.uses_legacy_syntax()
            && let Some(original) = rule.legacy_equipment_type.as_ref()
            && original.eq_ignore_ascii_case(&value)
        {
            // 选回同一材质时恢复旧单值的拼写与精确匹配，避免临时换选扩大规则。
            value.clone_from(original);
        }
        if let Some(selection) = rule.selection_mut(field) {
            if selection.conflicting_single.is_none() {
                selection.text = value;
            }
            return;
        }
        match field {
            "level_min" => rule.level_min = value,
            "level_max" => rule.level_max = value,
            "prefixes" => rule.prefixes = value,
            "suffixes" => rule.suffixes = value,
            "prefix_color" => rule.prefix_color = value,
            "suffix_color" => rule.suffix_color = value,
            "combined_color" => rule.combined_color = value,
            _ => {}
        }
    }

    pub(super) fn resolve_equipment_color_selection(
        &mut self,
        index: usize,
        field: &str,
        keep_single: bool,
    ) {
        if let Some(selection) =
            self.equipment_colors.get_mut(index).and_then(|rule| rule.selection_mut(field))
        {
            selection.resolve(keep_single);
        }
    }

    pub(super) fn signature(&self) -> Result<String, ()> {
        serde_json::to_string(&self.to_document()?).map_err(|_| ())
    }

    pub(super) fn edited_input(&self, path: &str) -> Option<&str> {
        self.input_edits.get(path).filter(|_| self.edits_field(path)).map(String::as_str)
    }

    /// 未接管的分组既不提交草稿输入，也不参与本地字段错误摘要。
    fn edits_field(&self, path: &str) -> bool {
        operational_fields::management_path(path)
            .is_none_or(|managed| path == managed || self.boolean(managed))
    }
}
