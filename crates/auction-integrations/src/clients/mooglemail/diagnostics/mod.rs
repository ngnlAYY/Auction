//! MoogleMail 敏感诊断证据的加密文件归档。

mod archive;
mod crypto;
mod filesystem;
mod payload;

pub use archive::{
    MoogleMailDiagnosticArchive, MoogleMailDiagnosticArchiveConfig, export_mooglemail_diagnostic,
};
pub use filesystem::{MailDiagnosticRetentionSnapshot, retention_snapshot};
