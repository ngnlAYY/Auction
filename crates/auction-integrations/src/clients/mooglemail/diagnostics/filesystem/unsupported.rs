//! 未提供安全文件权限与锁能力的平台显式禁用文件留证，不退化为明文保存。

use super::MoogleMailDiagnosticArchiveConfig;
use auction_application::MailDiagnosticError;
use std::path::Path;
use uuid::Uuid;

pub(super) fn persist(
    _: &MoogleMailDiagnosticArchiveConfig,
    _: Uuid,
    _: &[u8],
) -> Result<(), MailDiagnosticError> {
    Err(MailDiagnosticError::UnsupportedPlatform)
}
pub(super) fn read(
    _: &MoogleMailDiagnosticArchiveConfig,
    _: Uuid,
) -> Result<Vec<u8>, MailDiagnosticError> {
    Err(MailDiagnosticError::UnsupportedPlatform)
}
pub(super) fn write_private_export(_: &Path, _: &[u8]) -> Result<(), MailDiagnosticError> {
    Err(MailDiagnosticError::UnsupportedPlatform)
}
