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

pub trait AdminRuntimeFileStore: Send + Sync {
    /// 覆盖一次 runtime 更新/恢复全流程的跨进程排他锁。
    /// 锁必须使用非阻塞方式获取，并由返回的 RAII guard 持有到调用方完成 apply；
    /// `current`/`replace` 等单操作方法不能在 guard 存活期间再次获取同一把锁。
    type WorkflowGuard: Send + Sync;

    fn acquire_workflow_lock(&self) -> ApplicationFuture<'_, Self::WorkflowGuard>;

    /// 校验本地语法及启动时冻结的非敏感部署约束；不得读取文件、写命令账本或访问外部服务。
    /// cron 与有效 Worker 预算仍由配置 owner 提供；新候选缺少必要部署上下文时必须明确拒绝。
    fn validate_candidate(
        &self,
        document: &AdminRuntimeDocument,
        now: DateTime<Utc>,
    ) -> Result<(), ApplicationError>;

    fn current(&self) -> ApplicationFuture<'_, AdminRuntimeSnapshot>;

    /// 在调用方已经持有 workflow guard 时读取当前文件，避免递归获取文件锁。
    fn current_locked<'a>(
        &'a self,
        guard: &'a Self::WorkflowGuard,
    ) -> ApplicationFuture<'a, AdminRuntimeSnapshot>;

    /// 在调用方已经持有 workflow guard 时替换文件，避免递归获取文件锁。
    fn replace_locked<'a>(
        &'a self,
        guard: &'a Self::WorkflowGuard,
        expected_fingerprint: String,
        document: AdminRuntimeDocument,
    ) -> ApplicationFuture<'a, AdminRuntimeReplacement>;
}

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
