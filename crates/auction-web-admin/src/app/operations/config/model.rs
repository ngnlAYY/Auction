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

/// 页面保留服务端的完整安全投影，并只把可编辑标量转成适合浏览器输入的文本。
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct RuntimeConfigForm {
    document: Value,
    inputs: BTreeMap<String, String>,
    mailboxes: Vec<RuntimeMailboxForm>,
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
            let values = nested_value(document, path).and_then(Value::as_array).ok_or(())?;
            let text =
                values.iter().map(Value::as_str).collect::<Option<Vec<_>>>().ok_or(())?.join("\n");
            inputs.insert((*path).to_owned(), text);
        }
        for path in UNSIGNED_NUMBER_FIELDS.iter().chain(operational_fields::NUMBER_FIELDS) {
            let value = nested_value(document, path).and_then(Value::as_u64).ok_or(())?;
            inputs.insert((*path).to_owned(), value.to_string());
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
        Ok(Self { document: document.clone(), inputs, mailboxes })
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
