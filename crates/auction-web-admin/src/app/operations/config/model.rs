//! 运行时配置页面的安全表单状态与完整 JSON 投影转换。

use std::{collections::BTreeMap, rc::Rc};

use auction_contracts::{AdminRuntimeConfigResponse, EquipmentTaxonomyDto};
use serde_json::{Map, Number, Value};

use super::operational_fields;

const TEXT_FIELDS: &[&str] = &[
    "listener.address",
    "listener.group",
    "listener.mode",
    "listener.runtime_directory",
    "listener.socket_path",
    "mail_templates.auction_assets_rejected",
    "mail_templates.auction_equipment_rejected",
    "mail_templates.auction_no_attachments",
    "mail_templates.operations_alert",
    "mail_templates.rebind_result",
    "mail_templates.register_key",
    "mail_templates.register_result",
    "mail_templates.settlement_buyer_refund",
    "mail_templates.settlement_equipment_handoff",
    "mail_templates.settlement_payment_carrier",
    "mail_templates.settlement_payment_reminder",
    "mail_templates.settlement_seller_payout",
    "mail_templates.settlement_winner_equipment_cod",
    "mail_templates.user_assets_rejected",
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

/// 论坛装备标色规则的前端编辑状态；多选列表按行保存，兼容旧单值规则回填。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct RuntimeEquipmentColorForm {
    pub(super) category: String,
    pub(super) equipment_type: String,
    /// 旧材质单值保持精确匹配，用户主动改种类后才转换为新材质族多选。
    pub(super) legacy_equipment_type: Option<String>,
    pub(super) quality: String,
    pub(super) level_min: String,
    pub(super) level_max: String,
    pub(super) prefixes: String,
    pub(super) suffixes: String,
    pub(super) prefix_color: String,
    pub(super) suffix_color: String,
    pub(super) combined_color: String,
}

/// 页面保留服务端的完整安全投影，并只把可编辑标量转成适合浏览器输入的文本。
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct RuntimeConfigForm {
    document: Value,
    inputs: BTreeMap<String, String>,
    mailboxes: Vec<RuntimeMailboxForm>,
    equipment_colors: Vec<RuntimeEquipmentColorForm>,
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
            inputs.insert((*path).to_owned(), value.to_owned());
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
            inputs.insert((*path).to_owned(), text);
        }
        for path in UNSIGNED_NUMBER_FIELDS.iter().chain(operational_fields::NUMBER_FIELDS) {
            let value = nested_value(document, path).and_then(Value::as_u64);
            if let Some(value) = value {
                inputs.insert((*path).to_owned(), value.to_string());
            } else if *path == "operations.mail.batch_timeout_seconds"
                && nested_value(document, "operations.mail").is_some()
            {
                // 新字段对旧 runtime document 使用解析器默认值，避免历史文档无法打开。
                inputs.insert((*path).to_owned(), "600".to_owned());
            } else if path.starts_with("operations.mail.send_intervals.")
                && nested_value(document, "operations.mail").is_some()
                && nested_value(document, "operations.mail.send_intervals").is_none()
            {
                inputs.insert((*path).to_owned(), "0".to_owned());
            } else {
                return Err(());
            }
        }
        for path in SIGNED_NUMBER_FIELDS {
            let value = nested_value(document, path).and_then(Value::as_i64).ok_or(())?;
            inputs.insert((*path).to_owned(), value.to_string());
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
        if mailboxes.len() > 2 {
            return Err(());
        }
        let had_equipment_colors = nested_value(document, "equipment_colors").is_some();
        let equipment_colors = nested_value(document, "equipment_colors")
            .and_then(|value| value.get("rules"))
            .and_then(Value::as_array)
            .map(|rules| {
                rules
                    .iter()
                    .map(|rule| {
                        let object = rule.as_object().ok_or(())?;
                        Ok(RuntimeEquipmentColorForm {
                            category: color_selection_text(object, "categories", "category")?,
                            equipment_type: color_selection_text(
                                object,
                                "equipment_types",
                                "equipment_type",
                            )?,
                            legacy_equipment_type: legacy_material_selection(object, taxonomy),
                            quality: color_selection_text(object, "qualities", "quality")?,
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
            mailboxes,
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
                    rule.equipment_type.lines().any(|value| value.eq_ignore_ascii_case(&kind.name))
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
            && self.inputs == other.inputs
            && self.mailboxes == other.mailboxes
            && self.equipment_colors == other.equipment_colors
            && self.had_equipment_colors == other.had_equipment_colors
    }

    pub(super) fn input(&self, path: &str) -> String {
        self.inputs.get(path).cloned().unwrap_or_default()
    }

    pub(super) fn set_input(&mut self, path: &'static str, value: String) {
        self.inputs.insert(path.to_owned(), value);
    }

    pub(super) fn boolean(&self, path: &str) -> bool {
        nested_value(&self.document, path).and_then(Value::as_bool).unwrap_or(false)
    }

    pub(super) fn set_boolean(&mut self, path: &str, value: bool) {
        let _ = set_nested_value(&mut self.document, path, Value::Bool(value));
    }

    pub(super) fn mailboxes(&self) -> Vec<RuntimeMailboxForm> {
        self.mailboxes.clone()
    }

    pub(super) fn add_mailbox(&mut self) {
        if self.mailboxes.len() >= 2 {
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

    pub(super) fn equipment_colors(&self) -> Vec<RuntimeEquipmentColorForm> {
        self.equipment_colors.clone()
    }

    pub(super) fn add_equipment_color(&mut self) {
        self.equipment_colors.push(RuntimeEquipmentColorForm {
            category: String::new(),
            equipment_type: String::new(),
            legacy_equipment_type: None,
            quality: String::new(),
            level_min: "0".to_owned(),
            level_max: "0".to_owned(),
            prefixes: String::new(),
            suffixes: String::new(),
            prefix_color: "#ff0000".to_owned(),
            suffix_color: "#00aa00".to_owned(),
            combined_color: "#aa00aa".to_owned(),
        });
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

    pub(super) fn set_equipment_color_field(&mut self, index: usize, field: &str, value: String) {
        let Some(rule) = self.equipment_colors.get_mut(index) else { return };
        match field {
            "category" => rule.category = value,
            "equipment_type" => {
                if rule.equipment_type != value {
                    rule.legacy_equipment_type = None;
                }
                rule.equipment_type = value;
            }
            "quality" => rule.quality = value,
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

    /// 生成服务端要求的完整闭集文档；任一数字处于空白或非法中间态时拒绝提交。
    pub(super) fn to_document(&self) -> Result<Value, ()> {
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
            set_nested_value(&mut document, path, Value::String(self.required_input(path)?))?;
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

    pub(super) fn signature(&self) -> Result<String, ()> {
        serde_json::to_string(&self.to_document()?).map_err(|_| ())
    }

    fn required_input(&self, path: &str) -> Result<String, ()> {
        self.inputs.get(path).cloned().ok_or(())
    }

    /// 未接管的分组既不提交草稿输入，也不参与本地字段错误摘要。
    pub(super) fn edits_field(&self, path: &str) -> bool {
        operational_fields::management_path(path).is_none_or(|managed| self.boolean(managed))
    }
}

fn nested_value<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(root, |value, key| value.as_object()?.get(key))
}

fn set_nested_value(root: &mut Value, path: &str, value: Value) -> Result<(), ()> {
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

fn list_text(value: Option<&Value>) -> Result<String, ()> {
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
fn color_selection_text(
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

fn legacy_material_selection(
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

fn lines(value: &str) -> Vec<Value> {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| Value::String(line.to_owned()))
        .collect()
}
