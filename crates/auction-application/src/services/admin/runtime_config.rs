//! Admin runtime 配置应用服务。
//! 服务负责文档校验、指纹 fence、命令重放和 pending/unknown 恢复；文件和数据库的具体
//! 持久化仍分别由外层适配器实现。文件替换后不回退旧文档；账本提交结果未知时由后续恢复
//! 按实际状态重试或收口。

use std::sync::Arc;

use uuid::Uuid;

use crate::errors::ApplicationError;
use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, diagnostic_id, record_error,
};
use crate::ports::admin::{
    AdminRuntimeAuditRecord, AdminRuntimeCommandState, AdminRuntimeCommandStore,
    AdminRuntimeConfigError, AdminRuntimeConfigPort, AdminRuntimeDocument, AdminRuntimeFileStore,
    AdminRuntimeSnapshot, AdminRuntimeStoredCommand, AdminRuntimeUpdateCommand,
};
use crate::ports::core::ApplicationFuture;
use crate::ports::core::Clock;

pub struct AdminRuntimeConfigService<R, F, C> {
    commands: Arc<R>,
    files: Arc<F>,
    clock: Arc<C>,
}

impl<R, F, C> AdminRuntimeConfigService<R, F, C>
where
    R: AdminRuntimeCommandStore,
    F: AdminRuntimeFileStore,
    C: Clock,
{
    /// 建立绑定 durable 命令账本、TOML 文件适配器和时钟的服务。
    pub fn new(commands: Arc<R>, files: Arc<F>, clock: Arc<C>) -> Self {
        Self { commands, files, clock }
    }

    /// 读取当前文件快照；GET 不把重启要求伪装成热更新成功。
    pub async fn current(&self) -> Result<AdminRuntimeSnapshot, ApplicationError> {
        let mut snapshot = self.files.current().await?;
        snapshot.validate().map_err(ApplicationError::from)?;
        snapshot.restart_required = false;
        Ok(snapshot)
    }

    /// 严格校验候选文档且不触碰文件、数据库或外部服务。
    pub fn validate(
        &self,
        document: AdminRuntimeDocument,
    ) -> Result<AdminRuntimeSnapshot, ApplicationError> {
        document.validate().map_err(ApplicationError::from)?;
        self.files.validate_candidate(&document, self.clock.now())?;
        Ok(AdminRuntimeSnapshot {
            fingerprint: document.fingerprint(),
            document,
            restart_required: false,
        })
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
            // 同 ID 重试按落成规则比较：请求省略的兼容字段以已存候选补回后，摘要必须等于落成时
            // 的摘要。升级前写入的命令没有这些字段，补回后摘要不变。
            let replayed = command.document.clone().resolve_update(&stored.candidate_document);
            if !stored.matches_request(replayed.request_hash(&command.expected_fingerprint)?) {
                return Err(AdminRuntimeConfigError::CommandConflict.into());
            }
            return self.finish_existing(&guard, stored).await;
        }

        // 新命令在写 durable intent 前完成部署格式校验；已接受命令仍返回原有幂等结果。
        self.files.validate_candidate(&command.document, self.clock.now())?;
        let current = self.files.current_locked(&guard).await?;
        current.validate().map_err(ApplicationError::from)?;
        if current.fingerprint != command.expected_fingerprint {
            return Err(AdminRuntimeConfigError::StaleFingerprint.into());
        }
        // 请求省略的兼容字段（升级前的页面不认识）在指纹 fence 内按当前文件补回，保存不重置
        // 它们；命令摘要与新指纹都按落成的候选计算，账本行据此可校验、可重放。
        let candidate = command.document.resolve_update(&current.document);
        candidate.validate().map_err(ApplicationError::from)?;
        let request_hash = candidate.request_hash(&command.expected_fingerprint)?;

        let now = self.clock.now();
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
            let _ = self
                .commands
                .mark_unknown(
                    row.command_id,
                    row.request_hash,
                    "file_conflict".to_owned(),
                    self.clock.now(),
                )
                .await;
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
            let _ = self
                .commands
                .mark_unknown(
                    stored.command_id,
                    stored.request_hash,
                    "file_conflict".to_owned(),
                    self.clock.now(),
                )
                .await;
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

impl<R, F, C> AdminRuntimeConfigPort for AdminRuntimeConfigService<R, F, C>
where
    R: AdminRuntimeCommandStore + 'static,
    F: AdminRuntimeFileStore + 'static,
    C: Clock + 'static,
{
    fn current(&self) -> ApplicationFuture<'_, AdminRuntimeSnapshot> {
        Box::pin(async move { AdminRuntimeConfigService::current(self).await })
    }

    fn validate(
        &self,
        document: AdminRuntimeDocument,
    ) -> Result<AdminRuntimeSnapshot, ApplicationError> {
        AdminRuntimeConfigService::validate(self, document)
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

    use auction_domain::CommandId;
    use chrono::{DateTime, TimeZone, Utc};
    use uuid::Uuid;

    use super::AdminRuntimeConfigService;
    use crate::errors::ApplicationError;
    use crate::ports::admin::{
        AdminRuntimeAuditRecord, AdminRuntimeCommandState, AdminRuntimeCommandStore,
        AdminRuntimeConfigError, AdminRuntimeDocument, AdminRuntimeFileStore,
        AdminRuntimeReplacement, AdminRuntimeSnapshot, AdminRuntimeStoredCommand,
        AdminRuntimeUpdateCommand,
    };
    use crate::ports::core::{ApplicationFuture, Clock, RequestHash};

    struct FixedClock(DateTime<Utc>);

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    /// 内存中的 runtime 文件；替换按指纹 fence 校验。
    struct MemoryFiles(Mutex<AdminRuntimeDocument>);

    impl MemoryFiles {
        fn snapshot(&self) -> AdminRuntimeSnapshot {
            let document = self.0.lock().expect("文件锁").clone();
            AdminRuntimeSnapshot {
                fingerprint: document.fingerprint(),
                document,
                restart_required: false,
            }
        }
    }

    impl AdminRuntimeFileStore for MemoryFiles {
        type WorkflowGuard = ();

        fn acquire_workflow_lock(&self) -> ApplicationFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }

        fn validate_candidate(
            &self,
            _document: &AdminRuntimeDocument,
            _now: DateTime<Utc>,
        ) -> Result<(), ApplicationError> {
            Ok(())
        }

        fn current(&self) -> ApplicationFuture<'_, AdminRuntimeSnapshot> {
            Box::pin(async { Ok(self.snapshot()) })
        }

        fn current_locked<'a>(
            &'a self,
            _guard: &'a (),
        ) -> ApplicationFuture<'a, AdminRuntimeSnapshot> {
            Box::pin(async { Ok(self.snapshot()) })
        }

        fn replace_locked<'a>(
            &'a self,
            _guard: &'a (),
            expected_fingerprint: String,
            document: AdminRuntimeDocument,
        ) -> ApplicationFuture<'a, AdminRuntimeReplacement> {
            Box::pin(async move {
                let mut current = self.0.lock().expect("文件锁");
                let old_fingerprint = current.fingerprint();
                assert_eq!(old_fingerprint, expected_fingerprint, "替换必须按指纹 fence");
                let new_fingerprint = document.fingerprint();
                *current = document;
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
            applied_at: DateTime<Utc>,
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
            updated_at: DateTime<Utc>,
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
            updated_at: DateTime<Utc>,
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

    fn service(
        current: AdminRuntimeDocument,
    ) -> (AdminRuntimeConfigService<MemoryCommands, MemoryFiles, FixedClock>, Arc<MemoryFiles>)
    {
        let files = Arc::new(MemoryFiles(Mutex::new(current)));
        let clock = Utc.with_ymd_and_hms(2026, 11, 16, 0, 0, 0).single().expect("固定时刻");
        let service = AdminRuntimeConfigService::new(
            Arc::new(MemoryCommands::default()),
            Arc::clone(&files),
            Arc::new(FixedClock(clock)),
        );
        (service, files)
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

    /// 升级前的页面不认识长期未领取阈值：它保存的旧形状文档省略该键，保存不失败，也不把文件里
    /// 的 86400 重置为默认值；同一命令重试按原结果重放，不判为冲突。
    #[test]
    fn old_shape_update_keeps_the_current_collection_threshold() {
        let (service, files) = service(document_with_threshold(Some(86_400)));
        let expected = files.snapshot().fingerprint;
        let mut request = document_with_threshold(None);
        if let Some(operations) = &mut request.operations {
            operations.alerts.scan_interval_seconds = 120;
        }
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

        let replayed = block_on(service.update(command(command_id, &expected, request)))
            .expect("同一命令重试按原结果重放");
        assert_eq!(replayed.fingerprint, saved.fingerprint);
        assert_eq!(threshold(&replayed.document), Some(86_400));
    }

    /// 新页面显式写出默认值按省略保存（回退的旧程序仍能加载）；显式非默认值原样保存；同一命令
    /// ID 换成不同请求判为冲突。
    #[test]
    fn explicit_thresholds_are_saved_and_default_is_omitted() {
        let (service, files) = service(document_with_threshold(Some(86_400)));
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
        assert!(matches!(
            conflict,
            ApplicationError::AdminRuntime(AdminRuntimeConfigError::CommandConflict)
        ));

        let current = files.snapshot().fingerprint;
        let saved = block_on(service.update(command(
            CommandId::from_uuid(Uuid::from_u128(3)),
            &current,
            document_with_threshold(Some(3_600)),
        )))
        .expect("显式非默认值保存");
        assert_eq!(threshold(&saved.document), Some(3_600));
    }
}
