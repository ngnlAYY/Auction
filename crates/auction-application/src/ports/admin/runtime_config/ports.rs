use chrono::{DateTime, Utc};

use auction_domain::CommandId;

use crate::{
    ApplicationError,
    ports::{ApplicationFuture, RequestHash},
};

use super::{
    commands::{AdminRuntimeAuditRecord, AdminRuntimeReplacement, AdminRuntimeStoredCommand},
    document::{AdminRuntimeDocument, AdminRuntimeSnapshot},
};

/// runtime 文件适配器端口。
pub trait AdminRuntimeFileStore: Send + Sync {
    fn current(&self) -> ApplicationFuture<'_, AdminRuntimeSnapshot>;

    fn replace(
        &self,
        expected_fingerprint: String,
        document: AdminRuntimeDocument,
    ) -> ApplicationFuture<'_, AdminRuntimeReplacement>;

    fn restore(&self, replacement: AdminRuntimeReplacement) -> ApplicationFuture<'_, ()>;
}

/// runtime 命令账本适配器端口。
pub trait AdminRuntimeCommandStore: Send + Sync {
    fn find(
        &self,
        command_id: CommandId,
    ) -> ApplicationFuture<'_, Option<AdminRuntimeStoredCommand>>;

    fn list_unresolved(&self) -> ApplicationFuture<'_, Vec<AdminRuntimeStoredCommand>>;

    fn insert_pending(
        &self,
        command: AdminRuntimeStoredCommand,
    ) -> ApplicationFuture<'_, AdminRuntimeStoredCommand>;

    fn apply(
        &self,
        command_id: CommandId,
        request_hash: RequestHash,
        audit: AdminRuntimeAuditRecord,
        applied_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, AdminRuntimeStoredCommand>;

    fn mark_unknown(
        &self,
        command_id: CommandId,
        request_hash: RequestHash,
        category: String,
        updated_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, AdminRuntimeStoredCommand>;

    fn reopen(
        &self,
        command_id: CommandId,
        request_hash: RequestHash,
        updated_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, AdminRuntimeStoredCommand>;
}

/// Admin runtime 配置应用端口。
pub trait AdminRuntimeConfigPort: Send + Sync {
    fn current(&self) -> ApplicationFuture<'_, AdminRuntimeSnapshot>;

    fn validate(
        &self,
        document: AdminRuntimeDocument,
    ) -> Result<AdminRuntimeSnapshot, ApplicationError>;

    fn update(
        &self,
        command: super::commands::AdminRuntimeUpdateCommand,
    ) -> ApplicationFuture<'_, AdminRuntimeSnapshot>;
}
