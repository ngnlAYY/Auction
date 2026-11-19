//! runtime 文件存储、命令账本与配置用例端口。

use auction_domain::CommandId;
use auction_time::Timestamp;

use super::restart::{WorkerColdConfigDigest, WorkerRestartState};
use super::{
    commands::{AdminRuntimeAuditRecord, AdminRuntimeReplacement, AdminRuntimeStoredCommand},
    document::{AdminRuntimeDocument, AdminRuntimeSnapshot},
};
use crate::core::ApplicationError;
use crate::ports::core::ApplicationFuture;
use crate::ports::core::RequestHash;

pub trait AdminRuntimeFileStore: Send + Sync {
    /// 覆盖一次 runtime 更新/恢复全流程的跨进程排他锁。
    /// 锁必须使用非阻塞方式获取，并由返回的 RAII guard 持有到调用方完成 apply；
    /// `current`/`replace` 等单操作方法不能在 guard 存活期间再次获取同一把锁。
    type WorkflowGuard: Send + Sync;

    fn acquire_workflow_lock(&self) -> ApplicationFuture<'_, Self::WorkflowGuard>;

    /// 校验本地语法及非敏感部署约束；不得写文件、写命令账本或访问外部服务。
    fn validate_candidate(
        &self,
        document: &AdminRuntimeDocument,
        now: Timestamp,
    ) -> Result<(), ApplicationError>;

    /// 使用同一次加锁读取的快照校验落成候选。可取得可信 Worker 部署基值时总要校验候选；
    /// 无法取得时，只有未改变相关 Worker 覆盖的编辑可以继续。
    /// 不依赖当前文件的适配器可沿用本地校验。
    fn validate_candidate_against(
        &self,
        _current: &AdminRuntimeSnapshot,
        document: &AdminRuntimeDocument,
        now: Timestamp,
    ) -> Result<(), ApplicationError> {
        self.validate_candidate(document, now)
    }

    fn current(&self) -> ApplicationFuture<'_, AdminRuntimeSnapshot>;

    /// 用已读取的文档计算启动摘要；部署基值不可用时返回 None。
    fn worker_cold_config_target(
        &self,
        _snapshot: &AdminRuntimeSnapshot,
    ) -> Option<WorkerColdConfigDigest> {
        None
    }

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
        applied_at: Timestamp,
    ) -> ApplicationFuture<'_, AdminRuntimeStoredCommand>;

    fn mark_unknown(
        &self,
        command_id: CommandId,
        request_hash: RequestHash,
        category: String,
        updated_at: Timestamp,
    ) -> ApplicationFuture<'_, AdminRuntimeStoredCommand>;

    fn reopen(
        &self,
        command_id: CommandId,
        request_hash: RequestHash,
        updated_at: Timestamp,
    ) -> ApplicationFuture<'_, AdminRuntimeStoredCommand>;
}

pub trait AdminRuntimeConfigPort: Send + Sync {
    fn current(&self) -> ApplicationFuture<'_, AdminRuntimeSnapshot>;

    /// 使用返回给请求方的同一份文件快照比较 Worker 摘要。
    fn current_with_worker_restart_state(
        &self,
    ) -> ApplicationFuture<'_, (AdminRuntimeSnapshot, WorkerRestartState)> {
        Box::pin(async move { Ok((self.current().await?, WorkerRestartState::Unknown)) })
    }

    fn validate(
        &self,
        document: AdminRuntimeDocument,
    ) -> ApplicationFuture<'_, AdminRuntimeSnapshot>;

    fn update(
        &self,
        command: super::commands::AdminRuntimeUpdateCommand,
    ) -> ApplicationFuture<'_, AdminRuntimeSnapshot>;
}
