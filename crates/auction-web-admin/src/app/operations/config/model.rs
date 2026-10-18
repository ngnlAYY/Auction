//! 运行时配置页面的安全表单状态与完整 JSON 投影转换。

use std::collections::BTreeMap;

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

/// 论坛装备标色规则的前端编辑状态；数字和列表按文本保留中间态。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct RuntimeEquipmentColorForm {
    pub(super) category: String,
    pub(super) equipment_type: String,
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
}

impl RuntimeConfigForm {
    /// 读取编辑开始时的完整非敏感投影，供变更清单比对；不参与服务端授权。
    pub(super) fn source_document(&self) -> &Value {
        &self.document
    }

    pub(super) fn from_document(document: &Value) -> Result<Self, ()> {
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
                            category: object
                                .get("category")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            equipment_type: object
                                .get("equipment_type")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            quality: object
                                .get("quality")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
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
        })
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
            "equipment_type" => rule.equipment_type = value,
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
                    object.insert(
                        "category".to_owned(),
                        Value::String(rule.category.trim().to_owned()),
                    );
                    object.insert(
                        "equipment_type".to_owned(),
                        Value::String(rule.equipment_type.trim().to_owned()),
                    );
                    object.insert(
                        "quality".to_owned(),
                        Value::String(rule.quality.trim().to_owned()),
                    );
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

fn lines(value: &str) -> Vec<Value> {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| Value::String(line.to_owned()))
        .collect()
}
