//! 结算履约 owner 生成的无密文 MoogleMail 意图。

use super::super::*;

/// 结算履约 owner 生成的无密文 MoogleMail 意图。
///
/// 这里保留业务字段是为了让 transport 在外部写入前重新校验种类矩阵；装备 key 仍只存在
/// 数据库密文信封中，本次发送只使用同一快照中的远端装备 ID。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SettlementDeliveryPayload {
    pub(crate) schema_version: u8,
    pub(crate) operation: String,
    pub(crate) world: String,
    pub(crate) obligation_id: Uuid,
    pub(crate) auction_id: Uuid,
    pub(crate) lot_id: Uuid,
    pub(crate) origin_command_id: Uuid,
    pub(crate) kind: String,
    pub(crate) recipient: String,
    pub(crate) template: String,
    pub(crate) item_name: String,
    pub(crate) external_equipment_id: String,
    pub(crate) amount: i64,
    pub(crate) carrier_credit: u32,
    pub(crate) credits: i64,
    pub(crate) cod: i64,
    pub(crate) equipment_id: Uuid,
    pub(crate) submission_id: Option<Uuid>,
    pub(crate) submission_source_kind: Option<String>,
    pub(crate) source_external_mail_id: Option<String>,
}
