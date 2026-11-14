//! 旧版通用 MoogleMail schema-v1 结算载荷。

use serde::Deserialize;
use std::collections::BTreeMap;

/// send payload 中只保存 owner UUID 的装备 secret 引用。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SettlementEquipmentSecretReference {
    pub(crate) kind: String,
    pub(crate) id: String,
}

/// 旧版 BuildPayload 生成的 send 线协议。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SettlementSendWirePayload {
    pub(crate) schema_version: u8,
    pub(crate) operation: String,
    pub(crate) world: String,
    pub(crate) template: String,
    pub(crate) variables: BTreeMap<String, String>,
    pub(crate) credits: i64,
    pub(crate) cod: i64,
    pub(crate) equipment_secret_refs: Vec<SettlementEquipmentSecretReference>,
}

/// 仅用于识别并隔离历史记录的 return 线协议，禁止重新发送。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SettlementReturnWirePayload {
    pub(crate) schema_version: u8,
    pub(crate) operation: String,
    pub(crate) world: String,
    pub(crate) external_mail_id: String,
}

/// 完成严格 wire 校验后供 transport 使用的安全投影。
pub(crate) struct SettlementDeliveryPayload {
    pub(crate) operation: String,
    pub(crate) template: String,
    pub(crate) variables: BTreeMap<String, String>,
    pub(crate) credits: i64,
    pub(crate) cod: i64,
    pub(crate) equipment_secret_refs: Vec<SettlementEquipmentSecretReference>,
}
