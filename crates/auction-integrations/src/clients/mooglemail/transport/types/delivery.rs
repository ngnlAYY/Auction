//! 普通 MoogleMail delivery payload 与 take 前置资产期望。

use super::super::*;

/// PostgreSQL outbox 中版本化的 MoogleMail payload。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MailDeliveryPayload {
    pub(crate) schema_version: u8,
    pub(crate) operation: String,
    pub(crate) world: String,
    pub(crate) external_mail_id: String,
    pub(crate) template: Option<String>,
    pub(crate) result_code: Option<String>,
    pub(crate) body: Option<String>,
    pub(crate) user_key_user_id: Option<Uuid>,
    pub(crate) credits: i64,
    pub(crate) cod: i64,
    pub(crate) attachment_ids: Vec<String>,
}

/// take 动作必须在写入前核对的入站资产证据。
#[derive(Debug)]
pub(crate) struct ActionExpectation {
    pub(crate) credits: i64,
    pub(crate) cod: i64,
    pub(crate) attachment_ids: Vec<String>,
}
