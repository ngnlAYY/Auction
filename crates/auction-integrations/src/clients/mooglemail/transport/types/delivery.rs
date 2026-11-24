//! 普通 MoogleMail delivery payload 与 take 前置资产期望。

use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;

/// PostgreSQL outbox 中版本化的 MoogleMail payload。
///
/// 寄售处置（`origin_kind = "consignment_disposition"`）的载荷由投递通用外壳键与寄售
/// 专有键组成。外壳键为 `world`、`external_mail_id`、`template`、`result_code`、`body`、
/// `user_key_user_id`、`credits`、`cod`、`attachment_ids`，与其他投递来源相同；寄售专有键为
/// `schema_version = 2`、`operation`、`mail_key`、`decision_id`、`work_id`、`attempt_id`、
/// `expected_manifest`、`expected_return`。旧来源沿用 schema 1，且不得携带寄售专有键。
///
/// `mail_key` 必须与外壳的 `world` / `external_mail_id` 逐字一致；不一致时本模块的
/// `decode_payload` 失败关闭，数据库的 `c4_delivery_consignment_payload_ck` 强制同一约束。
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MailDeliveryPayload {
    /// 来源载荷版本；寄售处置固定为 2，旧来源为 1。
    pub schema_version: u8,
    /// 投递动作；寄售处置只允许 `take` / `return`。
    pub operation: String,
    /// 投递目标世界。
    pub world: String,
    /// 投递目标原信 MID。
    pub external_mail_id: String,
    /// 通知模板；寄售处置为空。
    pub template: Option<String>,
    /// 通知结果码；寄售处置为空。
    pub result_code: Option<String>,
    /// 通知正文；寄售处置为空。
    pub body: Option<String>,
    /// 用户密钥所属用户；寄售处置为空。
    pub user_key_user_id: Option<Uuid>,
    /// 附件之外的 Credits；寄售处置必须与冻结清单一致。
    pub credits: i64,
    /// COD 金额；寄售处置必须与冻结清单一致。
    pub cod: i64,
    /// 附件装备标识；寄售处置必须与冻结清单逐项一致。
    pub attachment_ids: Vec<String>,
    /// 寄售处置的原信键；旧来源必须为空。
    #[serde(default)]
    pub mail_key: Option<MailKeyPayload>,
    /// 寄售处置的最终决定；旧来源必须为空。
    #[serde(default)]
    pub decision_id: Option<Uuid>,
    /// 寄售处置的原工作；旧来源必须为空。
    #[serde(default)]
    pub work_id: Option<Uuid>,
    /// 寄售处置的本次 attempt；旧来源必须为空。
    #[serde(default)]
    pub attempt_id: Option<Uuid>,
    /// 寄售写前冻结清单；新来源必填。
    #[serde(default)]
    pub expected_manifest: Option<ExpectedMailManifestPayload>,
    /// Return 必填的写前退回证明；Take 与旧来源必须为空。
    #[serde(default)]
    pub expected_return: Option<ExpectedMailManifestPayload>,
}

/// 寄售处置绑定的原信键 `K=(bot_uid, world_id, external_mid)`。
///
/// `world_id` 必须与外壳 `world` 一致，`external_mid` 必须与外壳 `external_mail_id`
/// 一致；不一致时 4D1 解码失败，4M 的 `c4_delivery_consignment_payload_ck` 拒绝写入。
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MailKeyPayload {
    /// 认证 Bot UID 的规范正十进制文本。
    pub bot_uid: String,
    /// 原信世界；只允许 `main` / `isekai`。
    pub world_id: String,
    /// 原信 MID 的规范正十进制文本。
    pub external_mid: String,
}

/// 寄售写前清单的持久 wire 形状；正文只以摘要出现。
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExpectedMailManifestPayload {
    /// 原始发件人。
    pub sender: String,
    /// 原始收件人。
    pub recipient: String,
    /// 原始主题。
    pub subject: String,
    /// 64 位小写十六进制 SHA-256。
    pub body_digest: String,
    /// 清单摘要，64 位小写十六进制 SHA-256。
    pub manifest_digest: String,
    /// 附件之外的 Credits。
    pub credits: i64,
    /// COD 金额。
    pub cod: i64,
    /// 原始收件分钟文本。
    pub received_minute_raw: String,
    /// 按原始序号排列的附件。
    pub attachments: Vec<ExpectedAttachmentPayload>,
}

/// 冻结清单中的一件附件。
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExpectedAttachmentPayload {
    /// 原始附件序号，从 1 开始。
    pub ordinal: u32,
    /// 原始附件名称。
    pub name: String,
    /// 规范装备标识；无法识别时为空。
    pub canonical_eid: Option<String>,
}

/// take 动作必须在写入前核对的入站资产证据。
#[derive(Debug)]
pub(crate) struct ActionExpectation {
    pub(crate) credits: i64,
    pub(crate) cod: i64,
    pub(crate) attachment_ids: Vec<String>,
}
