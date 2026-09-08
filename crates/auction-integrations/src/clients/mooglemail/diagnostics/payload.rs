//! 加密载荷和公开认证头的闭集校验，以及敏感临时缓冲的清理。

use auction_application::{
    MailDiagnosticContext, MailDiagnosticError, MailDiagnosticOutcome, MailDiagnosticStage,
    MailMessage,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_PAYLOAD: usize = 8 * 1024 * 1024;
pub const FORMAT_VERSION: u8 = 2;

/// 临时明文不实现 Debug；正常和提前返回都会尽力清零当前分配。
pub(super) struct SensitiveBytes(pub(super) Vec<u8>);

impl Drop for SensitiveBytes {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

pub(super) fn clear_string(value: &mut String) {
    let mut bytes = std::mem::take(value).into_bytes();
    bytes.fill(0);
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContextWire {
    pub stage: String,
    pub operation: String,
    pub world_id: String,
    pub mailbox_key: Option<String>,
    pub external_mail_id: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OutcomeWire {
    pub failure_code: String,
    pub decision_code: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublicHeader {
    pub version: u8,
    pub evidence_id: uuid::Uuid,
    pub key_source: String,
    pub key_id: String,
    pub context: ContextWire,
    pub outcome: OutcomeWire,
    pub payload_kind: String,
    pub payload_bytes: usize,
    pub sha256: String,
    pub body_available: bool,
    pub status: Option<u16>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ParsedMessagePayload {
    pub received_at: String,
    pub sender: String,
    pub subject: String,
    pub body: String,
    pub credits: i64,
    pub cod: i64,
    pub attachments: Vec<(String, String)>,
    pub returnable: bool,
    pub claimable: bool,
    pub content_digest: String,
}

/// 按 MoogleMail v1 envelope 规则重算业务内容摘要，保持历史字节格式不变。
pub(super) fn content_digest(message: &MailMessage) -> Result<[u8; 32], MailDiagnosticError> {
    let mut digest = Sha256::new();
    write_digest_string(&mut digest, "mooglemail-envelope-v1");
    let received_at = message.received_at.to_rfc3339();
    let sender = message.sender.to_ascii_lowercase();
    for value in [
        message.external_mail_id.as_str(),
        message.world_id.as_str(),
        received_at.as_str(),
        sender.as_str(),
        message.subject.as_str(),
        message.body.as_str(),
    ] {
        write_digest_string(&mut digest, value);
    }
    digest.update(message.credits.to_be_bytes());
    digest.update(message.cod.to_be_bytes());
    digest.update([u8::from(message.returnable), u8::from(message.claimable)]);
    let mut equipment_ids = message.sorted_attachment_ids();
    equipment_ids.sort_unstable();
    for equipment_id in equipment_ids {
        write_digest_string(&mut digest, &equipment_id);
        let attachment = message
            .attachments
            .iter()
            .find(|attachment| attachment.external_equipment_id == equipment_id)
            .ok_or(MailDiagnosticError::InvalidMessage)?;
        attachment
            .key
            .with_bytes(|raw| digest.update(raw))
            .map_err(|_| MailDiagnosticError::InvalidMessage)?;
    }
    let bytes = digest.finalize();
    let mut result = [0_u8; 32];
    result.copy_from_slice(&bytes);
    Ok(result)
}

fn write_digest_string(digest: &mut Sha256, value: &str) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value.as_bytes());
}

impl Drop for ParsedMessagePayload {
    fn drop(&mut self) {
        clear_string(&mut self.sender);
        clear_string(&mut self.subject);
        clear_string(&mut self.body);
        for (_, key) in &mut self.attachments {
            clear_string(key);
        }
    }
}

pub fn context(c: &MailDiagnosticContext) -> Result<ContextWire, MailDiagnosticError> {
    if c.operation.is_empty()
        || c.operation.len() > 128
        || !code(&c.operation)
        || c.world_id.len() > 16
        || !matches!(c.world_id.as_str(), "main" | "isekai")
        || !valid_opt(&c.mailbox_key, 256)
        || !valid_opt(&c.external_mail_id, 512)
    {
        return Err(MailDiagnosticError::InvalidContext);
    }
    Ok(ContextWire {
        stage: match c.stage {
            MailDiagnosticStage::Transport => "transport",
            MailDiagnosticStage::Parse => "parse",
            MailDiagnosticStage::Business => "business",
        }
        .into(),
        operation: c.operation.clone(),
        world_id: c.world_id.clone(),
        mailbox_key: c.mailbox_key.clone(),
        external_mail_id: c.external_mail_id.clone(),
    })
}
pub fn outcome(o: &MailDiagnosticOutcome) -> Result<OutcomeWire, MailDiagnosticError> {
    if !code(&o.failure_code)
        || o.failure_code.len() > 128
        || o.decision_code.as_ref().is_some_and(|v| v.len() > 128 || !code(v))
    {
        return Err(MailDiagnosticError::InvalidOutcome);
    }
    Ok(OutcomeWire { failure_code: o.failure_code.clone(), decision_code: o.decision_code.clone() })
}
pub fn code(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}
fn valid_opt(v: &Option<String>, max: usize) -> bool {
    v.as_ref().is_none_or(|s| {
        !s.is_empty() && s.len() <= max && s == s.trim() && !s.chars().any(|c| c.is_control())
    })
}
pub fn validate_status(status: Option<u16>) -> Result<(), MailDiagnosticError> {
    if status.is_some_and(|s| !(100..=599).contains(&s)) {
        Err(MailDiagnosticError::InvalidContext)
    } else {
        Ok(())
    }
}
pub fn parsed_message(message: &MailMessage) -> Result<SensitiveBytes, MailDiagnosticError> {
    let mut value = ParsedMessagePayload {
        received_at: message.received_at.to_rfc3339(),
        sender: message.sender.clone(),
        subject: message.subject.clone(),
        body: message.body.clone(),
        credits: message.credits,
        cod: message.cod,
        attachments: Vec::with_capacity(message.attachments.len()),
        returnable: message.returnable,
        claimable: message.claimable,
        content_digest: super::crypto::hex(message.content_digest),
    };
    // EquipmentKey 的公开格式化会脱敏，只在此短生命周期加密边界内读取真实字节。
    for attachment in &message.attachments {
        let key = attachment
            .key
            .with_bytes(|raw| std::str::from_utf8(raw).map(str::to_owned))
            .map_err(|_| MailDiagnosticError::InvalidMessage)?
            .map_err(|_| MailDiagnosticError::InvalidMessage)?;
        value.attachments.push((attachment.external_equipment_id.clone(), key));
    }
    let result =
        SensitiveBytes(serde_json::to_vec(&value).map_err(|_| MailDiagnosticError::Serialization)?);
    if result.0.len() > MAX_PAYLOAD {
        return Err(MailDiagnosticError::PayloadTooLarge);
    }
    Ok(result)
}

/// 验证文件名身份及所有公开头字段；头部还必须通过 AEAD 认证才能被信任。
pub(super) fn validate_header(
    header: &PublicHeader,
    evidence_id: uuid::Uuid,
) -> Result<(), MailDiagnosticError> {
    let stage = match header.context.stage.as_str() {
        "transport" => MailDiagnosticStage::Transport,
        "parse" => MailDiagnosticStage::Parse,
        "business" => MailDiagnosticStage::Business,
        _ => return Err(MailDiagnosticError::Authentication),
    };
    context(&MailDiagnosticContext {
        stage,
        operation: header.context.operation.clone(),
        world_id: header.context.world_id.clone(),
        mailbox_key: header.context.mailbox_key.clone(),
        external_mail_id: header.context.external_mail_id.clone(),
    })?;
    outcome(&MailDiagnosticOutcome {
        failure_code: header.outcome.failure_code.clone(),
        decision_code: header.outcome.decision_code.clone(),
    })?;
    validate_status(header.status)?;
    if evidence_id.is_nil()
        || header.evidence_id != evidence_id
        || header.version != FORMAT_VERSION
        || !matches!(header.key_source.as_str(), "credential" | "equipment")
        || header.key_id.len() > 64
        || !code(&header.key_id)
        || header.payload_bytes > MAX_PAYLOAD
        || header.sha256.len() != 64
        || !header.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        || !matches!(header.payload_kind.as_str(), "raw_response" | "parsed_message")
        || (!header.body_available
            && (header.payload_bytes != 0 || header.payload_kind != "raw_response"))
        || (header.payload_kind == "parsed_message"
            && (header.status.is_some() || header.context.stage != "business"))
        || (header.payload_kind == "raw_response" && header.context.stage == "business")
    {
        return Err(MailDiagnosticError::Authentication);
    }
    Ok(())
}
