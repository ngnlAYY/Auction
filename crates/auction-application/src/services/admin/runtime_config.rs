//! Admin runtime 配置应用服务。
//! 服务负责文档校验、指纹 fence、命令重放和 pending/unknown 恢复；文件和数据库的具体
//! 持久化仍分别由外层适配器实现。文件替换后不回退旧文档；账本提交结果未知时由后续恢复
//! 按实际状态重试或收口。

use std::sync::Arc;

use auction_time::Timestamp;
use uuid::Uuid;

use crate::errors::ApplicationError;
use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, diagnostic_id, record_error,
};
use crate::ports::admin::{
    AdminRuntimeAuditRecord, AdminRuntimeCommandState, AdminRuntimeCommandStore,
    AdminRuntimeConfigError, AdminRuntimeConfigPort, AdminRuntimeDocument, AdminRuntimeFileStore,
    AdminRuntimeSnapshot, AdminRuntimeStoredCommand, AdminRuntimeUpdateCommand,
    WorkerColdConfigDigest, WorkerColdConfigReader, WorkerRestartState,
};
use crate::ports::core::ApplicationFuture;
use crate::ports::core::Clock;

pub struct AdminRuntimeConfigService<R, F, C> {
    commands: Arc<R>,
    files: Arc<F>,
    clock: Arc<C>,
    worker_cold_configs: Option<Arc<dyn WorkerColdConfigReader>>,
}

impl<R, F, C> AdminRuntimeConfigService<R, F, C>
where
    R: AdminRuntimeCommandStore,
    F: AdminRuntimeFileStore,
    C: Clock,
{
    /// 建立绑定 durable 命令账本、TOML 文件适配器和时钟的服务。
    pub fn new(commands: Arc<R>, files: Arc<F>, clock: Arc<C>) -> Self {
        Self { commands, files, clock, worker_cold_configs: None }
    }

    /// 装配 Worker 摘要只读端口；普通 GET 和历史命令回执不依赖此端口。
    pub fn with_worker_cold_configs(mut self, reader: Arc<dyn WorkerColdConfigReader>) -> Self {
        self.worker_cold_configs = Some(reader);
        self
    }

    async fn worker_restart_state(&self, snapshot: &AdminRuntimeSnapshot) -> WorkerRestartState {
        let Some((reader, target)) =
            self.worker_cold_configs.as_ref().zip(self.files.worker_cold_config_target(snapshot))
        else {
            return WorkerRestartState::Unknown;
        };
        match reader.live_worker_cold_configs().await {
            Ok(live) => compare_worker_cold_configs(&target, &live),
            Err(error) => {
                record_error(
                    DiagnosticLevel::Warn,
                    error.diagnostic_kind(),
                    "admin_runtime_config",
                    "read_worker_cold_configs",
                    DiagnosticContext::default(),
                );
                WorkerRestartState::Unknown
            }
        }
    }

    /// 读取当前文件快照；GET 不把重启要求伪装成热更新成功。
    pub async fn current(&self) -> Result<AdminRuntimeSnapshot, ApplicationError> {
        let mut snapshot = self.files.current().await?;
        snapshot.validate().map_err(ApplicationError::from)?;
        snapshot.restart_required = false;
        Ok(snapshot)
    }

    /// 严格校验候选文档；可读取部署配置，不写文件、命令账本或调用外部服务。
    pub async fn validate(
        &self,
        document: AdminRuntimeDocument,
    ) -> Result<AdminRuntimeSnapshot, ApplicationError> {
        let guard = self.files.acquire_workflow_lock().await?;
        let current = self.files.current_locked(&guard).await?;
        current.validate().map_err(ApplicationError::from)?;
        // 省略字段和 Worker 部署覆盖比较使用同一个锁内快照，避免并发请求混用文件版本。
        let document = self.validate_candidate(&current, document, self.clock.now())?;
        Ok(AdminRuntimeSnapshot {
            fingerprint: document.fingerprint(),
            document,
            restart_required: false,
        })
    }

    fn validate_candidate(
        &self,
        current: &AdminRuntimeSnapshot,
        document: AdminRuntimeDocument,
        now: Timestamp,
    ) -> Result<AdminRuntimeDocument, ApplicationError> {
        let candidate = document.resolve_update(&current.document);
        candidate.validate().map_err(ApplicationError::from)?;
        self.files.validate_candidate_against(current, &candidate, now)?;
        Ok(candidate)
    }

    /// 在 Admin surface 对外就绪前恢复尚未收口的 runtime 命令。
    /// 启动装配层负责决定哪些结果可以带着告警继续运行；服务层只负责执行与更新流程
    /// 相同的恢复逻辑，避免启动恢复和用户重试各自维护一套状态判断。
    pub async fn recover(&self) -> Result<(), ApplicationError> {
        let guard = self.files.acquire_workflow_lock().await?;
        self.recover_unresolved_locked(&guard).await
    }

    /// 执行一次带 durable intent 的 runtime 更新。
    pub async fn update(
        &self,
        command: AdminRuntimeUpdateCommand,
    ) -> Result<AdminRuntimeSnapshot, ApplicationError> {
        command.validate().map_err(ApplicationError::from)?;
        let guard = self.files.acquire_workflow_lock().await?;
        self.recover_unresolved_locked(&guard).await?;

        if let Some(stored) = self.commands.find(command.command_id).await? {
            stored.validate().map_err(ApplicationError::from)?;
            // 同 ID 重试只认原始请求（不变量 1）：不用已存候选补回请求省略的字段，省略与显式写出
            // （含默认值）判为不同请求；升级前的旧行沿用原有口径。相同则按账本状态收口，
            // 已应用的直接返回原结果，不再写文件。
            if !stored.replays(&command)? {
                return Err(AdminRuntimeConfigError::CommandConflict.into());
            }
            return self.finish_existing(&guard, stored).await;
        }

        let current = self.files.current_locked(&guard).await?;
        current.validate().map_err(ApplicationError::from)?;
        if current.fingerprint != command.expected_fingerprint {
            return Err(AdminRuntimeConfigError::StaleFingerprint.into());
        }
        // 请求省略的兼容字段（升级前的页面不认识）在指纹 fence 内按当前文件补回，保存不重置
        // 它们。命令身份是原始请求的摘要，落成候选的指纹另算、另存为 new_fingerprint；补回后的
        // 候选不充当请求身份，否则同 ID 的省略请求会被已存候选补成显式请求而误判为重放。
        let request_hash = command.request_hash;

        let now = self.clock.now();
        // 写 durable intent 前按锁内回读的当前文件校验最终候选；已接受命令仍按账本重放。
        let candidate = self.validate_candidate(&current, command.document, now)?;
        let stored = self
            .commands
            .insert_pending(AdminRuntimeStoredCommand {
                command_id: command.command_id,
                audit_id: Uuid::new_v4(),
                digest_version: crate::ports::admin::ADMIN_RUNTIME_DIGEST_VERSION,
                request_hash,
                expected_fingerprint: command.expected_fingerprint,
                old_fingerprint: current.fingerprint,
                new_fingerprint: candidate.fingerprint(),
                candidate_document: candidate,
                result_document: None,
                restart_required: true,
                actor_id: command.actor_id,
                request_id: command.request_id,
                occurred_at: now,
                state: AdminRuntimeCommandState::Pending,
                unknown_category: None,
                created_at: now,
                updated_at: now,
                applied_at: None,
            })
            .await?;
        stored.validate().map_err(ApplicationError::from)?;
        self.finish_pending(&guard, stored).await
    }

    async fn recover_unresolved_locked(
        &self,
        guard: &F::WorkflowGuard,
    ) -> Result<(), ApplicationError> {
        let rows = self.commands.list_unresolved().await?;
        if rows.len() > 1 {
            return Err(AdminRuntimeConfigError::StoredResultCorrupt.into());
        }
        let Some(row) = rows.into_iter().next() else {
            return Ok(());
        };
        row.validate().map_err(ApplicationError::from)?;
        let current = self.files.current_locked(guard).await?;
        current.validate().map_err(ApplicationError::from)?;
        if current.fingerprint == row.new_fingerprint {
            self.apply_verified(guard, row).await.map(|_| ())
        } else if current.fingerprint == row.old_fingerprint {
            if row.state == AdminRuntimeCommandState::Unknown {
                let reopened = self
                    .commands
                    .reopen(row.command_id, row.request_hash, self.clock.now())
                    .await?;
                self.finish_pending(guard, reopened).await.map(|_| ())
            } else {
                // durable intent 已记录且旧文件仍可证明；继续同一命令，不能永久占用唯一 pending 槽。
                self.finish_pending(guard, row).await.map(|_| ())
            }
        } else {
            self.commands
                .mark_unknown(
                    row.command_id,
                    row.request_hash,
                    "file_conflict".to_owned(),
                    self.clock.now(),
                )
                .await?;
            Err(AdminRuntimeConfigError::ReconciliationRequired.into())
        }
    }

    async fn finish_existing(
        &self,
        guard: &F::WorkflowGuard,
        stored: AdminRuntimeStoredCommand,
    ) -> Result<AdminRuntimeSnapshot, ApplicationError> {
        match stored.state {
            AdminRuntimeCommandState::Applied => self.finish_applied(stored),
            AdminRuntimeCommandState::Pending => self.finish_pending(guard, stored).await,
            AdminRuntimeCommandState::Unknown => {
                Err(AdminRuntimeConfigError::ReconciliationRequired.into())
            }
        }
    }

    async fn finish_pending(
        &self,
        guard: &F::WorkflowGuard,
        stored: AdminRuntimeStoredCommand,
    ) -> Result<AdminRuntimeSnapshot, ApplicationError> {
        if stored.state != AdminRuntimeCommandState::Pending {
            return Err(AdminRuntimeConfigError::StoredResultCorrupt.into());
        }
        let current = self.files.current_locked(guard).await?;
        current.validate().map_err(ApplicationError::from)?;
        if current.fingerprint == stored.new_fingerprint {
            return self.apply_verified(guard, stored).await;
        }
        if current.fingerprint != stored.old_fingerprint {
            self.commands
                .mark_unknown(
                    stored.command_id,
                    stored.request_hash,
                    "file_conflict".to_owned(),
                    self.clock.now(),
                )
                .await?;
            return Err(AdminRuntimeConfigError::ReconciliationRequired.into());
        }

        let replacement = self
            .files
            .replace_locked(
                guard,
                stored.old_fingerprint.clone(),
                stored.candidate_document.clone(),
            )
            .await?;
        if replacement.old_fingerprint != stored.old_fingerprint
            || replacement.new_fingerprint != stored.new_fingerprint
        {
            return self.mark_unknown(stored, "result_corrupt").await;
        }

        match self.apply_row(&stored).await {
            Ok(applied) => self.finish_applied(applied),
            // 文件已经替换成功；不能因数据库错误而恢复旧文件。错误可能发生在事务提交
            // 后，恢复旧文件会把 Applied 账本与实际配置永久分离。保留新文件后，若事务
            // 回滚，pending 行会在下一次恢复中重新应用；若已提交，则直接收口。
            Err(error) => {
                record_error(
                    DiagnosticLevel::Warn,
                    error.diagnostic_kind(),
                    "admin_runtime_config",
                    "persist_applied",
                    DiagnosticContext::default(),
                );
                Err(AdminRuntimeConfigError::OutcomeUnknown.into())
            }
        }
    }

    async fn apply_verified(
        &self,
        guard: &F::WorkflowGuard,
        stored: AdminRuntimeStoredCommand,
    ) -> Result<AdminRuntimeSnapshot, ApplicationError> {
        let current = self.files.current_locked(guard).await?;
        current.validate().map_err(ApplicationError::from)?;
        if current.fingerprint != stored.new_fingerprint
            || stored.candidate_document.fingerprint() != stored.new_fingerprint
        {
            return Err(AdminRuntimeConfigError::StoredResultCorrupt.into());
        }
        // 当前文件已经证明是候选版本；账本错误的提交结果未知，必须保留 durable intent，
        // 让下一次恢复依据数据库实际状态收口，不能把底层错误伪装成可安全回退的失败。
        let applied = self.apply_row(&stored).await.map_err(|error| {
            record_error(
                DiagnosticLevel::Warn,
                error.diagnostic_kind(),
                "admin_runtime_config",
                "recover_application",
                DiagnosticContext {
                    command_id: diagnostic_id(&stored.command_id),
                    ..Default::default()
                },
            );
            AdminRuntimeConfigError::OutcomeUnknown
        })?;
        self.finish_applied(applied)
    }

    async fn apply_row(
        &self,
        stored: &AdminRuntimeStoredCommand,
    ) -> Result<AdminRuntimeStoredCommand, ApplicationError> {
        self.commands
            .apply(
                stored.command_id,
                stored.request_hash,
                AdminRuntimeAuditRecord {
                    id: stored.audit_id,
                    occurred_at: stored.occurred_at,
                    actor_id: stored.actor_id.clone(),
                    request_id: stored.request_id.clone(),
                    command_id: stored.command_id,
                    old_fingerprint: stored.old_fingerprint.clone(),
                    new_fingerprint: stored.new_fingerprint.clone(),
                },
                self.clock.now(),
            )
            .await
    }

    async fn mark_unknown(
        &self,
        stored: AdminRuntimeStoredCommand,
        category: &str,
    ) -> Result<AdminRuntimeSnapshot, ApplicationError> {
        let _ = self
            .commands
            .mark_unknown(
                stored.command_id,
                stored.request_hash,
                category.to_owned(),
                self.clock.now(),
            )
            .await?;
        Err(AdminRuntimeConfigError::OutcomeUnknown.into())
    }

    fn finish_applied(
        &self,
        stored: AdminRuntimeStoredCommand,
    ) -> Result<AdminRuntimeSnapshot, ApplicationError> {
        let document =
            stored.result_document.ok_or(AdminRuntimeConfigError::StoredResultCorrupt)?;
        let snapshot = AdminRuntimeSnapshot {
            fingerprint: stored.new_fingerprint,
            document,
            restart_required: true,
        };
        snapshot.validate().map_err(ApplicationError::from)?;
        Ok(snapshot)
    }
}

fn compare_worker_cold_configs(
    target: &WorkerColdConfigDigest,
    live: &[Option<WorkerColdConfigDigest>],
) -> WorkerRestartState {
    let mut complete = !live.is_empty();
    for digest in live {
        match digest.as_ref().filter(|digest| digest.comparable_to(target)) {
            Some(digest) if digest != target => return WorkerRestartState::RestartRequired,
            Some(_) => {}
            None => complete = false,
        }
    }
    if complete { WorkerRestartState::Consistent } else { WorkerRestartState::Unknown }
}

impl<R, F, C> AdminRuntimeConfigPort for AdminRuntimeConfigService<R, F, C>
where
    R: AdminRuntimeCommandStore + 'static,
    F: AdminRuntimeFileStore + 'static,
    C: Clock + 'static,
{
    fn current(&self) -> ApplicationFuture<'_, AdminRuntimeSnapshot> {
        Box::pin(async move { AdminRuntimeConfigService::current(self).await })
    }

    fn current_with_worker_restart_state(
        &self,
    ) -> ApplicationFuture<'_, (AdminRuntimeSnapshot, WorkerRestartState)> {
        Box::pin(async move {
            let snapshot = self.current().await?;
            // 比较与响应始终使用同一份文档和指纹；无需持有文件锁等待数据库。
            let state = self.worker_restart_state(&snapshot).await;
            Ok((snapshot, state))
        })
    }

    fn validate(
        &self,
        document: AdminRuntimeDocument,
    ) -> ApplicationFuture<'_, AdminRuntimeSnapshot> {
        Box::pin(async move { AdminRuntimeConfigService::validate(self, document).await })
    }

    fn update(
        &self,
        command: AdminRuntimeUpdateCommand,
    ) -> ApplicationFuture<'_, AdminRuntimeSnapshot> {
        Box::pin(async move { AdminRuntimeConfigService::update(self, command).await })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
    use std::time::Duration;

    use auction_domain::CommandId;
    use auction_time::Timestamp;
    use uuid::Uuid;

    use super::{AdminRuntimeConfigService, compare_worker_cold_configs};
    use crate::errors::ApplicationError;
    use crate::ports::admin::{
        ADMIN_RUNTIME_LEGACY_DIGEST_VERSION, AdminRuntimeAuditRecord, AdminRuntimeCommandState,
        AdminRuntimeCommandStore, AdminRuntimeConfigError, AdminRuntimeDocument,
        AdminRuntimeFileError, AdminRuntimeFileStore, AdminRuntimeReplacement,
        AdminRuntimeSnapshot, AdminRuntimeStoredCommand, AdminRuntimeUpdateCommand,
    };
    use crate::ports::core::{ApplicationFuture, Clock, RequestHash};

    #[test]
    fn worker_restart_state_requires_complete_comparable_evidence() {
        use crate::ports::admin::{WorkerColdConfigDigest, WorkerRestartState};

        let target = WorkerColdConfigDigest::from_projection(1, &"new").unwrap();
        let older = WorkerColdConfigDigest::from_projection(1, &"old").unwrap();
        let future = WorkerColdConfigDigest::from_projection(2, &"new").unwrap();
        let malformed =
            serde_json::from_value(serde_json::json!({"version": 1, "sha256": "invalid"})).unwrap();

        for (live, expected) in [
            (vec![], WorkerRestartState::Unknown),
            (vec![None], WorkerRestartState::Unknown),
            (vec![Some(future), Some(target.clone())], WorkerRestartState::Unknown),
            (vec![Some(malformed)], WorkerRestartState::Unknown),
            (vec![Some(target.clone()), None], WorkerRestartState::Unknown),
            (vec![Some(older.clone())], WorkerRestartState::RestartRequired),
            (vec![Some(target.clone())], WorkerRestartState::Consistent),
            (vec![Some(target.clone()), Some(target.clone())], WorkerRestartState::Consistent),
            (vec![Some(target.clone()), Some(older.clone())], WorkerRestartState::RestartRequired),
            (vec![None, Some(older), Some(target.clone())], WorkerRestartState::RestartRequired),
        ] {
            assert_eq!(compare_worker_cold_configs(&target, &live), expected, "{live:?}");
        }
    }

    struct FixedClock(Timestamp);

    impl Clock for FixedClock {
        fn now(&self) -> Timestamp {
            self.0
        }
    }

    /// 内存中的 runtime 文件；替换按指纹 fence 校验，并计数替换次数，供断言重放不回写文件。
    struct MemoryFiles {
        document: Mutex<AdminRuntimeDocument>,
        writes: AtomicUsize,
        locked: Arc<AtomicBool>,
        lock_acquisitions: AtomicUsize,
        validation_pause: Mutex<Option<ValidationPause>>,
    }

    struct ValidationPause {
        entered: SyncSender<String>,
        resume: Receiver<()>,
    }

    struct MemoryWorkflowGuard(Arc<AtomicBool>);

    impl Drop for MemoryWorkflowGuard {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }

    impl MemoryFiles {
        fn snapshot(&self) -> AdminRuntimeSnapshot {
            let document = self.document.lock().expect("文件锁").clone();
            AdminRuntimeSnapshot {
                fingerprint: document.fingerprint(),
                document,
                restart_required: false,
            }
        }

        fn writes(&self) -> usize {
            self.writes.load(Ordering::SeqCst)
        }
    }

    impl AdminRuntimeFileStore for MemoryFiles {
        type WorkflowGuard = MemoryWorkflowGuard;

        fn acquire_workflow_lock(&self) -> ApplicationFuture<'_, MemoryWorkflowGuard> {
            Box::pin(async {
                if self.locked.swap(true, Ordering::SeqCst) {
                    return Err(
                        AdminRuntimeFileError::Unavailable("workflow 已加锁".to_owned()).into()
                    );
                }
                self.lock_acquisitions.fetch_add(1, Ordering::SeqCst);
                Ok(MemoryWorkflowGuard(Arc::clone(&self.locked)))
            })
        }

        fn validate_candidate(
            &self,
            _document: &AdminRuntimeDocument,
            _now: Timestamp,
        ) -> Result<(), ApplicationError> {
            Ok(())
        }

        fn validate_candidate_against(
            &self,
            current: &AdminRuntimeSnapshot,
            _document: &AdminRuntimeDocument,
            _now: Timestamp,
        ) -> Result<(), ApplicationError> {
            let pause = self.validation_pause.lock().unwrap().take();
            if let Some(ValidationPause { entered, resume }) = pause {
                entered.send(current.fingerprint.clone()).unwrap();
                resume.recv_timeout(Duration::from_secs(10)).unwrap();
            }
            assert!(self.locked.load(Ordering::SeqCst));
            assert_eq!(current.fingerprint, self.snapshot().fingerprint);
            Ok(())
        }

        fn current(&self) -> ApplicationFuture<'_, AdminRuntimeSnapshot> {
            Box::pin(async {
                let guard = self.acquire_workflow_lock().await?;
                self.current_locked(&guard).await
            })
        }

        fn current_locked<'a>(
            &'a self,
            _guard: &'a MemoryWorkflowGuard,
        ) -> ApplicationFuture<'a, AdminRuntimeSnapshot> {
            Box::pin(async { Ok(self.snapshot()) })
        }

        fn replace_locked<'a>(
            &'a self,
            _guard: &'a MemoryWorkflowGuard,
            expected_fingerprint: String,
            document: AdminRuntimeDocument,
        ) -> ApplicationFuture<'a, AdminRuntimeReplacement> {
            Box::pin(async move {
                let mut current = self.document.lock().expect("文件锁");
                let old_fingerprint = current.fingerprint();
                assert_eq!(old_fingerprint, expected_fingerprint, "替换必须按指纹 fence");
                let new_fingerprint = document.fingerprint();
                *current = document;
                self.writes.fetch_add(1, Ordering::SeqCst);
                Ok(AdminRuntimeReplacement { old_fingerprint, new_fingerprint })
            })
        }
    }

    /// 内存命令账本：一行一个命令 ID。
    #[derive(Default)]
    struct MemoryCommands(Mutex<Vec<AdminRuntimeStoredCommand>>);

    impl MemoryCommands {
        fn update(
            &self,
            command_id: CommandId,
            change: impl FnOnce(&mut AdminRuntimeStoredCommand),
        ) -> AdminRuntimeStoredCommand {
            let mut rows = self.0.lock().expect("账本锁");
            let row =
                rows.iter_mut().find(|row| row.command_id == command_id).expect("账本行必须存在");
            change(row);
            row.clone()
        }

        /// 直接放入一行，模拟升级前的程序已写下的账本。
        fn seed(&self, row: AdminRuntimeStoredCommand) {
            self.0.lock().expect("账本锁").push(row);
        }
    }

    impl AdminRuntimeCommandStore for MemoryCommands {
        fn find(
            &self,
            command_id: CommandId,
        ) -> ApplicationFuture<'_, Option<AdminRuntimeStoredCommand>> {
            let row = self
                .0
                .lock()
                .expect("账本锁")
                .iter()
                .find(|row| row.command_id == command_id)
                .cloned();
            Box::pin(async move { Ok(row) })
        }

        fn list_unresolved(&self) -> ApplicationFuture<'_, Vec<AdminRuntimeStoredCommand>> {
            let rows = self
                .0
                .lock()
                .expect("账本锁")
                .iter()
                .filter(|row| row.state != AdminRuntimeCommandState::Applied)
                .cloned()
                .collect();
            Box::pin(async move { Ok(rows) })
        }

        fn insert_pending(
            &self,
            command: AdminRuntimeStoredCommand,
        ) -> ApplicationFuture<'_, AdminRuntimeStoredCommand> {
            self.0.lock().expect("账本锁").push(command.clone());
            Box::pin(async move { Ok(command) })
        }

        fn apply(
            &self,
            command_id: CommandId,
            _request_hash: RequestHash,
            _audit: AdminRuntimeAuditRecord,
            applied_at: Timestamp,
        ) -> ApplicationFuture<'_, AdminRuntimeStoredCommand> {
            let row = self.update(command_id, |row| {
                row.state = AdminRuntimeCommandState::Applied;
                row.result_document = Some(row.candidate_document.clone());
                row.applied_at = Some(applied_at);
                row.updated_at = applied_at;
            });
            Box::pin(async move { Ok(row) })
        }

        fn mark_unknown(
            &self,
            command_id: CommandId,
            _request_hash: RequestHash,
            category: String,
            updated_at: Timestamp,
        ) -> ApplicationFuture<'_, AdminRuntimeStoredCommand> {
            let row = self.update(command_id, |row| {
                row.state = AdminRuntimeCommandState::Unknown;
                row.unknown_category = Some(category);
                row.updated_at = updated_at;
            });
            Box::pin(async move { Ok(row) })
        }

        fn reopen(
            &self,
            command_id: CommandId,
            _request_hash: RequestHash,
            updated_at: Timestamp,
        ) -> ApplicationFuture<'_, AdminRuntimeStoredCommand> {
            let row = self.update(command_id, |row| {
                row.state = AdminRuntimeCommandState::Pending;
                row.unknown_category = None;
                row.updated_at = updated_at;
            });
            Box::pin(async move { Ok(row) })
        }
    }

    fn document_with_threshold(seconds: Option<u64>) -> AdminRuntimeDocument {
        let mut document = AdminRuntimeDocument::default();
        document.materialize_business_defaults();
        if let Some(operations) = &mut document.operations {
            operations.alerts.fulfillment_collection_overdue_age_seconds = seconds;
        }
        document
    }

    fn threshold(document: &AdminRuntimeDocument) -> Option<u64> {
        document
            .operations
            .as_ref()
            .and_then(|operations| operations.alerts.fulfillment_collection_overdue_age_seconds)
    }

    /// 另有一处编辑（扫描间隔 120 秒）的请求，阈值按参数省略或显式写出。
    fn edited_request(seconds: Option<u64>) -> AdminRuntimeDocument {
        let mut request = document_with_threshold(seconds);
        if let Some(operations) = &mut request.operations {
            operations.alerts.scan_interval_seconds = 120;
        }
        request
    }

    fn command(
        command_id: CommandId,
        expected_fingerprint: &str,
        document: AdminRuntimeDocument,
    ) -> AdminRuntimeUpdateCommand {
        let request_hash = document.request_hash(expected_fingerprint).expect("请求摘要");
        AdminRuntimeUpdateCommand {
            command_id,
            expected_fingerprint: expected_fingerprint.to_owned(),
            document,
            actor_id: "admin".to_owned(),
            request_id: None,
            request_hash,
        }
    }

    /// 升级前的程序写下的已应用账本行（摘要版本 1）：摘要按候选计算。
    fn legacy_row(
        command_id: CommandId,
        expected_fingerprint: &str,
        candidate: AdminRuntimeDocument,
    ) -> AdminRuntimeStoredCommand {
        let at = Timestamp::from_ymd_hms(2026, 11, 1, 0, 0, 0).expect("固定时刻");
        AdminRuntimeStoredCommand {
            command_id,
            audit_id: Uuid::new_v4(),
            digest_version: ADMIN_RUNTIME_LEGACY_DIGEST_VERSION,
            request_hash: candidate.legacy_request_hash(expected_fingerprint).expect("旧版摘要"),
            expected_fingerprint: expected_fingerprint.to_owned(),
            old_fingerprint: expected_fingerprint.to_owned(),
            new_fingerprint: candidate.fingerprint(),
            result_document: Some(candidate.clone()),
            candidate_document: candidate,
            restart_required: true,
            actor_id: "admin".to_owned(),
            request_id: None,
            occurred_at: at,
            state: AdminRuntimeCommandState::Applied,
            unknown_category: None,
            created_at: at,
            updated_at: at,
            applied_at: Some(at),
        }
    }

    type MemoryService = AdminRuntimeConfigService<MemoryCommands, MemoryFiles, FixedClock>;

    fn service(
        current: AdminRuntimeDocument,
    ) -> (MemoryService, Arc<MemoryFiles>, Arc<MemoryCommands>) {
        let files = Arc::new(MemoryFiles {
            document: Mutex::new(current),
            writes: AtomicUsize::new(0),
            locked: Arc::new(AtomicBool::new(false)),
            lock_acquisitions: AtomicUsize::new(0),
            validation_pause: Mutex::new(None),
        });
        let commands = Arc::new(MemoryCommands::default());
        let clock = Timestamp::from_ymd_hms(2026, 11, 16, 0, 0, 0).expect("固定时刻");
        let service = AdminRuntimeConfigService::new(
            Arc::clone(&commands),
            Arc::clone(&files),
            Arc::new(FixedClock(clock)),
        );
        (service, files, commands)
    }

    fn is_conflict(error: &ApplicationError) -> bool {
        matches!(error, ApplicationError::AdminRuntime(AdminRuntimeConfigError::CommandConflict))
    }

    /// 只轮询立即就绪的内存端口，不引入额外异步运行时依赖。
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(waker);
        let mut future = Box::pin(future);
        loop {
            match future.as_mut().poll(&mut context) {
                std::task::Poll::Ready(value) => return value,
                std::task::Poll::Pending => std::hint::spin_loop(),
            }
        }
    }

    #[test]
    fn preview_resolves_omitted_fields_from_the_current_file() {
        let (service, files, _) = service(document_with_threshold(Some(86_400)));
        let preview = block_on(service.validate(edited_request(None))).unwrap();
        assert_eq!(threshold(&preview.document), Some(86_400));
        assert_eq!(preview.fingerprint, preview.document.fingerprint());
        assert_eq!(files.writes(), 0);
    }

    #[test]
    fn preview_holds_one_snapshot_until_validation_finishes() {
        let (service, files, commands) = service(document_with_threshold(Some(86_400)));
        let original = files.snapshot().fingerprint;
        let command_id = CommandId::from_uuid(Uuid::from_u128(1));
        let save = || command(command_id, &original, edited_request(Some(3_600)));
        let (entered, validating) = sync_channel(1);
        let (resume, resumed) = sync_channel(1);
        *files.validation_pause.lock().unwrap() =
            Some(ValidationPause { entered, resume: resumed });

        let (preview, competing_save, seen_fingerprint) = std::thread::scope(|scope| {
            let preview = scope.spawn(|| block_on(service.validate(edited_request(None))));
            let seen_fingerprint = validating.recv_timeout(Duration::from_secs(10)).unwrap();
            let competing_save = block_on(service.update(save()));
            resume.send(()).unwrap();
            (preview.join().unwrap().unwrap(), competing_save, seen_fingerprint)
        });
        assert_eq!(seen_fingerprint, original);
        assert_eq!(threshold(&preview.document), Some(86_400));
        assert_eq!(files.lock_acquisitions.load(Ordering::SeqCst), 1);
        assert!(matches!(
            competing_save,
            Err(ApplicationError::AdminRuntimeFile(AdminRuntimeFileError::Unavailable(_)))
        ));
        assert_eq!(files.writes(), 0);
        assert!(block_on(commands.find(command_id)).unwrap().is_none());

        let saved = block_on(service.update(save())).unwrap();
        let next = block_on(service.validate(edited_request(None))).unwrap();
        assert_eq!(threshold(&next.document), Some(3_600));
        assert_eq!(next.fingerprint, saved.fingerprint);
        assert_eq!(files.writes(), 1);
    }

    /// 升级前的页面不认识长期未领取阈值：它保存的旧形状文档省略该键，保存不失败，也不把文件里
    /// 的 86400 重置为默认值；同一命令重试按原结果重放，不判为冲突，也不再写文件。
    #[test]
    fn old_shape_update_keeps_the_current_collection_threshold() {
        let (service, files, _) = service(document_with_threshold(Some(86_400)));
        let expected = files.snapshot().fingerprint;
        let request = edited_request(None);
        let command_id = CommandId::from_uuid(Uuid::from_u128(1));
        let saved = block_on(service.update(command(command_id, &expected, request.clone())))
            .expect("旧形状保存必须成功");
        assert_eq!(threshold(&saved.document), Some(86_400), "省略表示保持当前值");
        assert_eq!(
            saved
                .document
                .operations
                .as_ref()
                .map(|operations| operations.alerts.scan_interval_seconds),
            Some(120),
            "其余编辑照常生效"
        );
        assert_eq!(files.snapshot().document, saved.document, "文件即落成的候选");
        assert_eq!(files.snapshot().fingerprint, saved.fingerprint);
        assert_eq!(files.writes(), 1);

        let replayed = block_on(service.update(command(command_id, &expected, request)))
            .expect("同一命令重试按原结果重放");
        assert_eq!(replayed, saved);
        assert_eq!(files.writes(), 1, "重放不回写文件");
    }

    /// 新页面显式写出默认值按省略保存（回退的旧程序仍能加载）；显式非默认值原样保存；同一命令
    /// ID 换成不同请求判为冲突。
    #[test]
    fn explicit_thresholds_are_saved_and_default_is_omitted() {
        let (service, files, _) = service(document_with_threshold(Some(86_400)));
        let expected = files.snapshot().fingerprint;
        let command_id = CommandId::from_uuid(Uuid::from_u128(2));
        let saved = block_on(service.update(command(
            command_id,
            &expected,
            document_with_threshold(Some(259_200)),
        )))
        .expect("显式默认值保存");
        assert_eq!(threshold(&saved.document), None, "显式默认值按省略写入文件");
        assert_eq!(saved.fingerprint, document_with_threshold(None).fingerprint());

        let conflict = block_on(service.update(command(
            command_id,
            &expected,
            document_with_threshold(Some(3_600)),
        )))
        .expect_err("同 ID 不同请求必须冲突");
        assert!(is_conflict(&conflict));

        let current = files.snapshot().fingerprint;
        let saved = block_on(service.update(command(
            CommandId::from_uuid(Uuid::from_u128(3)),
            &current,
            document_with_threshold(Some(3_600)),
        )))
        .expect("显式非默认值保存");
        assert_eq!(threshold(&saved.document), Some(3_600));
    }

    /// 不变量 1：同一命令 ID 下，省略阈值与显式写出阈值是不同请求，显式写出默认值或与当前文件
    /// 相同的值也一样，先后顺序无关；不能用已存候选把省略补成显式值后当作重放。冲突不写文件。
    #[test]
    fn omitted_and_explicit_thresholds_are_different_requests() {
        for (id, first, second) in [
            (10, Some(3_600), None),
            (11, Some(259_200), None),
            (12, None, Some(259_200)),
            (13, None, Some(86_400)),
        ] {
            let (service, files, _) = service(document_with_threshold(Some(86_400)));
            let expected = files.snapshot().fingerprint;
            let command_id = CommandId::from_uuid(Uuid::from_u128(id));
            let saved = block_on(service.update(command(
                command_id,
                &expected,
                document_with_threshold(first),
            )))
            .expect("首个请求保存");
            // 省略阈值时候选与当前文件相同，首次保存不必替换文件；只断言冲突前后没有新写入。
            let writes = files.writes();
            let conflict = block_on(service.update(command(
                command_id,
                &expected,
                document_with_threshold(second),
            )))
            .expect_err("同 ID 不同请求必须冲突");
            assert!(is_conflict(&conflict), "{first:?} → {second:?}：{conflict:?}");
            assert_eq!(files.writes(), writes, "{first:?} → {second:?}：冲突不写文件");
            assert_eq!(files.snapshot().document, saved.document, "文件保持首个请求的结果");
        }
    }

    /// 完全相同的请求（省略或显式）同 ID 重放返回原结果且不回写；其他命令改过文件后，旧命令同 ID
    /// 重放仍返回它当时的结果，不回写文件，也不覆盖后来的修改。
    #[test]
    fn identical_replays_return_the_original_result_without_rewriting() {
        let (service, files, _) = service(document_with_threshold(Some(86_400)));
        let initial = files.snapshot().fingerprint;
        let omitted = CommandId::from_uuid(Uuid::from_u128(20));
        let first = block_on(service.update(command(omitted, &initial, edited_request(None))))
            .expect("省略阈值保存");
        assert_eq!(threshold(&first.document), Some(86_400));

        let explicit = CommandId::from_uuid(Uuid::from_u128(21));
        let second = block_on(service.update(command(
            explicit,
            &first.fingerprint,
            document_with_threshold(Some(3_600)),
        )))
        .expect("显式阈值保存");
        assert_eq!(threshold(&files.snapshot().document), Some(3_600));
        assert_eq!(files.writes(), 2);

        for (command_id, expected, request, original) in [
            (omitted, &initial, edited_request(None), &first),
            (explicit, &first.fingerprint, document_with_threshold(Some(3_600)), &second),
        ] {
            let replayed = block_on(service.update(command(command_id, expected, request)))
                .expect("原样重试返回原结果");
            assert_eq!(&replayed, original);
        }
        assert_eq!(files.writes(), 2, "重放不回写文件");
        assert_eq!(files.snapshot().document, second.document, "后来的修改保持不变");
    }

    /// 升级前写下的账本行（摘要版本 1）按写入时的口径重放：原样重试返回原结果、不回写文件，
    /// 显式写出不同阈值仍冲突。覆盖两种旧行：升级前程序写的（候选即请求，不含阈值）与按补回后
    /// 候选计算摘要的（请求省略阈值，候选含当时文件的 86400）。
    #[test]
    fn legacy_ledger_rows_replay_by_their_original_rule() {
        let (service, files, commands) = service(document_with_threshold(Some(3_600)));
        let request = edited_request(None);

        let pre_upgrade_file = document_with_threshold(None).fingerprint();
        let pre_upgrade = CommandId::from_uuid(Uuid::from_u128(30));
        commands.seed(legacy_row(pre_upgrade, &pre_upgrade_file, request.clone()));
        let replayed =
            block_on(service.update(command(pre_upgrade, &pre_upgrade_file, request.clone())))
                .expect("旧命令原样重试返回原结果");
        assert_eq!(replayed.document, request);
        assert_eq!(replayed.fingerprint, request.fingerprint());
        // 旧口径不变：显式写出的默认值按省略落成，与旧候选相同，仍判为同一请求。
        block_on(service.update(command(
            pre_upgrade,
            &pre_upgrade_file,
            edited_request(Some(259_200)),
        )))
        .expect("旧口径下显式默认值与省略落成相同");
        let conflict = block_on(service.update(command(
            pre_upgrade,
            &pre_upgrade_file,
            edited_request(Some(3_600)),
        )))
        .expect_err("显式非默认值不同于旧请求");
        assert!(is_conflict(&conflict));

        let resolved_from = document_with_threshold(Some(86_400));
        let candidate = request.clone().resolve_update(&resolved_from);
        let resolved = CommandId::from_uuid(Uuid::from_u128(31));
        commands.seed(legacy_row(resolved, &resolved_from.fingerprint(), candidate.clone()));
        let replayed =
            block_on(service.update(command(resolved, &resolved_from.fingerprint(), request)))
                .expect("省略阈值的原样重试按旧口径补回后相同");
        assert_eq!(replayed.document, candidate);
        assert_eq!(threshold(&replayed.document), Some(86_400));

        assert_eq!(files.writes(), 0, "旧命令重放不回写文件");
        assert_eq!(threshold(&files.snapshot().document), Some(3_600), "当前文件保持不变");
    }
}
