//! 证据文件格式与平台边界；按最终落盘字节计算容量。

use auction_application::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use auction_application::MailDiagnosticError;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use std::{
    path::{Component, Path},
    time::Duration,
};
use uuid::Uuid;

use super::{
    MoogleMailDiagnosticArchiveConfig,
    payload::{self, PublicHeader},
};

#[cfg(unix)]
#[path = "filesystem/unix.rs"]
mod platform;
#[cfg(not(unix))]
#[path = "filesystem/unsupported.rs"]
mod platform;

const MAX_HEADER_BYTES: usize = 8 * 1024;
const MAX_CIPHERTEXT_BYTES: usize = payload::MAX_PAYLOAD + 12 + 16;
const MAX_FILE_BYTES: usize = MAX_CIPHERTEXT_BYTES.div_ceil(3) * 4 + MAX_HEADER_BYTES + 1;

pub(super) fn validate_config(
    config: &MoogleMailDiagnosticArchiveConfig,
) -> Result<(), MailDiagnosticError> {
    if config.directory.as_os_str().is_empty()
        || config.directory.file_name().is_none()
        || config.directory.to_string_lossy().chars().any(char::is_control)
        || matches!(
            config.directory.components().next_back(),
            Some(Component::CurDir | Component::ParentDir | Component::RootDir)
        )
        || config.retention < Duration::from_secs(86_400)
        || config.retention > Duration::from_secs(365 * 86_400)
        || !(16 * 1024 * 1024..=1024 * 1024 * 1024).contains(&config.maximum_total_bytes)
        || !matches!(config.key_source.as_str(), "credential" | "equipment")
    {
        return Err(MailDiagnosticError::InvalidContext);
    }
    Ok(())
}

pub(super) fn persist(
    config: &MoogleMailDiagnosticArchiveConfig,
    header: &PublicHeader,
    ciphertext: &[u8],
) -> Result<(), MailDiagnosticError> {
    validate_config(config)?;
    payload::validate_header(header, header.evidence_id)?;
    let mut encoded = serde_json::to_string(header).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "encode_record_header",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Serialization
    })?;
    if encoded.len() > MAX_HEADER_BYTES || ciphertext.len() > MAX_CIPHERTEXT_BYTES {
        return Err(MailDiagnosticError::PayloadTooLarge);
    }
    encoded.push('\n');
    // 直接追加到最终文件缓冲，避免先分配并复制整份 Base64 密文；文件字节格式保持不变。
    BASE64.encode_string(ciphertext, &mut encoded);
    if encoded.len() > MAX_FILE_BYTES || encoded.len() as u64 > config.maximum_total_bytes {
        return Err(MailDiagnosticError::Capacity);
    }
    platform::persist(config, header.evidence_id, encoded.as_bytes())
}

pub(super) fn read(
    config: &MoogleMailDiagnosticArchiveConfig,
    id: Uuid,
) -> Result<(PublicHeader, Vec<u8>), MailDiagnosticError> {
    validate_config(config)?;
    if id.is_nil() {
        return Err(MailDiagnosticError::InvalidContext);
    }
    let encoded = platform::read(config, id)?;
    let split = encoded
        .iter()
        .position(|byte| *byte == b'\n')
        .filter(|split| *split <= MAX_HEADER_BYTES)
        .ok_or(MailDiagnosticError::Authentication)?;
    let header: PublicHeader =
        serde_json::from_slice(&encoded[..split]).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "decode_record_header",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Authentication
        })?;
    payload::validate_header(&header, id)?;
    let ciphertext = BASE64.decode(&encoded[split + 1..]).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "decode_record_ciphertext",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Authentication
    })?;
    if ciphertext.len() > MAX_CIPHERTEXT_BYTES {
        return Err(MailDiagnosticError::PayloadTooLarge);
    }
    Ok((header, ciphertext))
}

pub(super) fn write_private_export(output: &Path, bytes: &[u8]) -> Result<(), MailDiagnosticError> {
    platform::write_private_export(output, bytes)
}
