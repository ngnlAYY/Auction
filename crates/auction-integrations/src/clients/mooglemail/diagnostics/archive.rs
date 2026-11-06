//! 异常证据的准备、认证归档及离线导出，不参与邮件业务决定。

use auction_application::core::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use std::{path::PathBuf, sync::Arc, time::Duration};

use auction_application::mail::{
    MailDiagnosticContext, MailDiagnosticError, MailDiagnosticFuture, MailDiagnosticOutcome,
    MailDiagnosticReceipt, MailDiagnosticRecorder, MailMessage, PreparedMailEvidence,
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    crypto, filesystem,
    payload::{self, SensitiveBytes},
};
use crate::keyring::Keyring;

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
    keys: Arc<Keyring>,
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
        keys: Arc<Keyring>,
    ) -> Result<Self, MailDiagnosticError> {
        filesystem::validate_config(&config)?;
        if keys.domain().as_str() != config.key_source {
            return Err(MailDiagnosticError::InvalidContext);
        }
        Ok(Self { config, keys })
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
        let digest = auction_application::core::lower_hex(&Sha256::digest(&body.0));
        let header = payload::PublicHeader {
            version: payload::FORMAT_VERSION,
            evidence_id: id,
            key_source: self.config.key_source.clone(),
            key_id: self.keys.current_id().to_owned(),
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
        let encrypted = crypto::seal(self.keys.current(), &self.config.key_source, &aad, &body.0)?;
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
            key_id: self.keys.current_id().to_owned(),
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
            self.keys.current(),
            &self.config.key_source,
            &authenticated_header(&header)?,
            prepared.ciphertext(),
        )?;
        verify_payload(&header, &body.0)?;
        let final_header = payload::PublicHeader { outcome: payload::outcome(&outcome)?, ..header };
        let ciphertext = crypto::seal(
            self.keys.current(),
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
                .map_err(|diagnostic_cause| {
                    safe_diagnostics::error(
                        "mooglemail_diagnostics",
                        "retain_join",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                    MailDiagnosticError::Io
                })?
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
                .map_err(|diagnostic_cause| {
                    safe_diagnostics::error(
                        "mooglemail_diagnostics",
                        "retain_response_join",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                    MailDiagnosticError::Io
                })?
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
    keyrings: Vec<Arc<Keyring>>,
    evidence_id: Uuid,
    output: PathBuf,
) -> Result<(), MailDiagnosticError> {
    let (header, ciphertext) = filesystem::read(&config, evidence_id)?;
    payload::validate_header(&header, evidence_id)?;
    // 归档头部记录写入时的域；只在该域的密钥环内解析旧 key ID，不跨域回退。
    let keys = keyrings
        .iter()
        .find(|keys| keys.domain().as_str() == header.key_source)
        .ok_or(MailDiagnosticError::Authentication)?;
    let data_key = keys.resolve(&header.key_id).map_err(|error| {
        let _ = error;
        MailDiagnosticError::Authentication
    })?;
    let plain =
        crypto::open(data_key, &header.key_source, &authenticated_header(&header)?, &ciphertext)?;
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
    let bytes = SensitiveBytes(serde_json::to_vec_pretty(&export).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "encode_export",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Serialization
    })?);
    filesystem::write_private_export(&output, &bytes.0)
}

fn authenticated_header(header: &payload::PublicHeader) -> Result<Vec<u8>, MailDiagnosticError> {
    let encoded = serde_json::to_vec(header).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "encode_authenticated_header",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Serialization
    })?;
    Ok(crypto::aad(&encoded))
}

fn verify_payload(header: &payload::PublicHeader, body: &[u8]) -> Result<(), MailDiagnosticError> {
    if body.len() != header.payload_bytes
        || auction_application::core::lower_hex(&Sha256::digest(body)) != header.sha256
    {
        return Err(MailDiagnosticError::Authentication);
    }
    Ok(())
}
