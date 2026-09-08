//! 异常证据的准备、认证归档及离线导出，不参与邮件业务决定。

use std::{path::PathBuf, sync::Arc, time::Duration};

use auction_application::{
    MailDiagnosticContext, MailDiagnosticError, MailDiagnosticFuture, MailDiagnosticOutcome,
    MailDiagnosticReceipt, MailDiagnosticRecorder, MailEquipmentAttachment, MailMessage,
    PreparedMailEvidence,
};
use auction_domain::EquipmentKey;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    crypto, filesystem,
    payload::{self, SensitiveBytes},
};
use crate::DataKey;

/// 本地加密证据的目录、清理预算和父密钥来源。
#[derive(Debug, Clone)]
pub struct MoogleMailDiagnosticArchiveConfig {
    pub directory: PathBuf,
    pub retention: Duration,
    pub maximum_total_bytes: u64,
    pub key_source: String,
}

/// 使用独立域派生子密钥的文件适配器；所有落盘动作由文件锁串行化。
#[derive(Clone)]
pub struct MoogleMailDiagnosticArchive {
    config: MoogleMailDiagnosticArchiveConfig,
    data_key: Arc<DataKey>,
}

/// 此明文类型只用于显式导出，不实现 Debug，销毁时清理正文副本。
#[derive(Serialize)]
struct DiagnosticExport {
    evidence_id: Uuid,
    context: payload::ContextWire,
    outcome: payload::OutcomeWire,
    body_available: bool,
    status: Option<u16>,
    payload_kind: String,
    payload_base64: Option<String>,
    payload_text: Option<String>,
}

impl Drop for DiagnosticExport {
    fn drop(&mut self) {
        for value in [&mut self.payload_base64, &mut self.payload_text].into_iter().flatten() {
            payload::clear_string(value);
        }
    }
}

impl MoogleMailDiagnosticArchive {
    /// 校验归档配置；不创建目录、不删除历史证据，也不改变业务配置。
    pub fn new(
        config: MoogleMailDiagnosticArchiveConfig,
        data_key: Arc<DataKey>,
    ) -> Result<Self, MailDiagnosticError> {
        filesystem::validate_config(&config)?;
        Ok(Self { config, data_key })
    }

    /// 读取并认证 parsed_message 证据，直接恢复为短生命周期 MailMessage。
    ///
    /// 该入口只信任归档文件中的 AEAD 认证头和 payload digest，不读取或信任外部导出的 JSON。
    pub fn read_parsed_message(
        &self,
        evidence_id: Uuid,
    ) -> Result<MailMessage, MailDiagnosticError> {
        let (header, ciphertext) = filesystem::read(&self.config, evidence_id)?;
        payload::validate_header(&header, evidence_id)?;
        if header.key_source != self.config.key_source
            || header.key_id != self.data_key.id()
            || header.payload_kind != "parsed_message"
            || !header.body_available
        {
            return Err(MailDiagnosticError::Authentication);
        }
        let plain = crypto::open(
            &self.data_key,
            &self.config.key_source,
            &authenticated_header(&header)?,
            &ciphertext,
        )?;
        verify_payload(&header, &plain.0)?;
        let mut value: payload::ParsedMessagePayload =
            serde_json::from_slice(&plain.0).map_err(|_| MailDiagnosticError::Serialization)?;
        let mailbox_key =
            header.context.mailbox_key.clone().ok_or(MailDiagnosticError::Authentication)?;
        let external_mail_id =
            header.context.external_mail_id.clone().ok_or(MailDiagnosticError::Authentication)?;
        let received_at = DateTime::parse_from_rfc3339(&value.received_at)
            .map_err(|_| MailDiagnosticError::InvalidMessage)?
            .with_timezone(&Utc);
        let mut digest = [0_u8; 32];
        decode_hex_digest(&value.content_digest, &mut digest)?;
        let mut raw_attachments = std::mem::take(&mut value.attachments);
        let mut attachments = Vec::with_capacity(raw_attachments.len());
        while let Some((external_equipment_id, mut raw_key)) = raw_attachments.pop() {
            let parsed_key = EquipmentKey::new(&raw_key);
            payload::clear_string(&mut raw_key);
            let Ok(key) = parsed_key else {
                for (_, mut remaining_key) in raw_attachments.drain(..) {
                    payload::clear_string(&mut remaining_key);
                }
                return Err(MailDiagnosticError::InvalidMessage);
            };
            attachments.push(MailEquipmentAttachment { external_equipment_id, key });
        }
        let sender = std::mem::take(&mut value.sender);
        let subject = std::mem::take(&mut value.subject);
        let body = std::mem::take(&mut value.body);
        let message = MailMessage {
            mailbox_key,
            external_mail_id,
            world_id: header.context.world_id,
            received_at,
            sender,
            subject,
            body,
            credits: value.credits,
            cod: value.cod,
            attachments,
            returnable: value.returnable,
            claimable: value.claimable,
            content_digest: digest,
        };
        message.validate().map_err(|_| MailDiagnosticError::InvalidMessage)?;
        if payload::content_digest(&message)? != digest {
            return Err(MailDiagnosticError::Authentication);
        }
        Ok(message)
    }

    fn prepare(
        &self,
        context: MailDiagnosticContext,
        kind: &str,
        body: SensitiveBytes,
        status: Option<u16>,
        available: bool,
    ) -> Result<PreparedMailEvidence, MailDiagnosticError> {
        let context_wire = payload::context(&context)?;
        let id = Uuid::new_v4();
        let digest = crypto::hex(Sha256::digest(&body.0));
        let header = payload::PublicHeader {
            version: payload::FORMAT_VERSION,
            evidence_id: id,
            key_source: self.config.key_source.clone(),
            key_id: self.data_key.id().to_owned(),
            context: context_wire,
            outcome: payload::OutcomeWire { failure_code: "prepared".into(), decision_code: None },
            payload_kind: kind.into(),
            payload_bytes: body.0.len(),
            sha256: digest.clone(),
            body_available: available,
            status,
        };
        payload::validate_header(&header, id)?;
        let aad = authenticated_header(&header)?;
        let encrypted = crypto::seal(&self.data_key, &self.config.key_source, &aad, &body.0)?;
        let prepared = PreparedMailEvidence::new(id, context, encrypted, body.0.len(), digest);
        Ok(if kind == "raw_response" {
            prepared.with_response_metadata(status, available)
        } else {
            prepared
        })
    }

    fn retain_blocking(
        &self,
        prepared: PreparedMailEvidence,
        outcome: MailDiagnosticOutcome,
    ) -> Result<MailDiagnosticReceipt, MailDiagnosticError> {
        let header = payload::PublicHeader {
            version: payload::FORMAT_VERSION,
            evidence_id: prepared.evidence_id(),
            key_source: self.config.key_source.clone(),
            key_id: self.data_key.id().to_owned(),
            context: payload::context(prepared.context())?,
            outcome: payload::OutcomeWire { failure_code: "prepared".into(), decision_code: None },
            payload_kind: prepared.payload_kind().to_owned(),
            payload_bytes: prepared.plaintext_bytes(),
            sha256: prepared.sha256().into(),
            body_available: prepared.body_available(),
            status: prepared.status(),
        };
        payload::validate_header(&header, prepared.evidence_id())?;
        let body = crypto::open(
            &self.data_key,
            &self.config.key_source,
            &authenticated_header(&header)?,
            prepared.ciphertext(),
        )?;
        verify_payload(&header, &body.0)?;
        let final_header = payload::PublicHeader { outcome: payload::outcome(&outcome)?, ..header };
        let ciphertext = crypto::seal(
            &self.data_key,
            &self.config.key_source,
            &authenticated_header(&final_header)?,
            &body.0,
        )?;
        filesystem::persist(&self.config, &final_header, &ciphertext)?;
        Ok(MailDiagnosticReceipt {
            evidence_id: prepared.evidence_id(),
            plaintext_bytes: prepared.plaintext_bytes(),
            sha256: prepared.sha256().to_owned(),
            body_available: final_header.body_available,
        })
    }
}

fn decode_hex_digest(value: &str, output: &mut [u8; 32]) -> Result<(), MailDiagnosticError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(MailDiagnosticError::Authentication);
    }
    for (index, pair) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        output[index] = (hex_nibble(pair[0]) << 4) | hex_nibble(pair[1]);
    }
    Ok(())
}

fn hex_nibble(value: u8) -> u8 {
    match value {
        b'0'..=b'9' => value - b'0',
        b'a'..=b'f' => value - b'a' + 10,
        b'A'..=b'F' => value - b'A' + 10,
        _ => 0,
    }
}

impl MailDiagnosticRecorder for MoogleMailDiagnosticArchive {
    fn prepare_message(
        &self,
        context: MailDiagnosticContext,
        message: &MailMessage,
    ) -> Result<PreparedMailEvidence, MailDiagnosticError> {
        if message.validate().is_err() {
            return Err(MailDiagnosticError::InvalidMessage);
        }
        self.prepare(context, "parsed_message", payload::parsed_message(message)?, None, true)
    }

    fn retain(
        &self,
        prepared: PreparedMailEvidence,
        outcome: MailDiagnosticOutcome,
    ) -> MailDiagnosticFuture<'_, MailDiagnosticReceipt> {
        let archive = self.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || archive.retain_blocking(prepared, outcome))
                .await
                .map_err(|_| MailDiagnosticError::Io)?
        })
    }

    fn retain_response(
        &self,
        context: MailDiagnosticContext,
        status: Option<u16>,
        body: Option<&[u8]>,
        outcome: MailDiagnosticOutcome,
    ) -> MailDiagnosticFuture<'_, MailDiagnosticReceipt> {
        let prepared = self.prepare_response(context, status, body);
        let archive = self.clone();
        Box::pin(async move {
            let prepared = prepared?;
            tokio::task::spawn_blocking(move || archive.retain_blocking(prepared, outcome))
                .await
                .map_err(|_| MailDiagnosticError::Io)?
        })
    }

    fn prepare_response(
        &self,
        context: MailDiagnosticContext,
        status: Option<u16>,
        body: Option<&[u8]>,
    ) -> Result<PreparedMailEvidence, MailDiagnosticError> {
        if body.is_some_and(|bytes| bytes.len() > payload::MAX_PAYLOAD) {
            return Err(MailDiagnosticError::PayloadTooLarge);
        }
        let available = body.is_some();
        let bytes = SensitiveBytes(body.unwrap_or_default().to_vec());
        self.prepare(context, "raw_response", bytes, status, available)
    }
}

/// 按配置目录中的 UUID 验证、解密并导出到全新私有文件，绝不向 stdout 输出正文。
pub fn export_mooglemail_diagnostic(
    config: MoogleMailDiagnosticArchiveConfig,
    data_key: Arc<DataKey>,
    evidence_id: Uuid,
    output: PathBuf,
) -> Result<(), MailDiagnosticError> {
    let (header, ciphertext) = filesystem::read(&config, evidence_id)?;
    payload::validate_header(&header, evidence_id)?;
    if header.key_source != config.key_source || header.key_id != data_key.id() {
        return Err(MailDiagnosticError::Authentication);
    }
    let plain =
        crypto::open(&data_key, &config.key_source, &authenticated_header(&header)?, &ciphertext)?;
    verify_payload(&header, &plain.0)?;
    let (payload_base64, payload_text) = if header.body_available {
        (Some(BASE64.encode(&plain.0)), std::str::from_utf8(&plain.0).ok().map(str::to_owned))
    } else {
        (None, None)
    };
    let export = DiagnosticExport {
        evidence_id,
        context: header.context,
        outcome: header.outcome,
        body_available: header.body_available,
        status: header.status,
        payload_kind: header.payload_kind,
        payload_base64,
        payload_text,
    };
    let bytes = SensitiveBytes(
        serde_json::to_vec_pretty(&export).map_err(|_| MailDiagnosticError::Serialization)?,
    );
    filesystem::write_private_export(&output, &bytes.0)
}

fn authenticated_header(header: &payload::PublicHeader) -> Result<Vec<u8>, MailDiagnosticError> {
    let encoded = serde_json::to_vec(header).map_err(|_| MailDiagnosticError::Serialization)?;
    Ok(crypto::aad(&encoded))
}

fn verify_payload(header: &payload::PublicHeader, body: &[u8]) -> Result<(), MailDiagnosticError> {
    if body.len() != header.payload_bytes || crypto::hex(Sha256::digest(body)) != header.sha256 {
        return Err(MailDiagnosticError::Authentication);
    }
    Ok(())
}
