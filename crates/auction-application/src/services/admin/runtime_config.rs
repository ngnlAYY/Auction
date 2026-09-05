//! Admin runtime 配置应用服务。
//!
//! 服务负责文档校验、指纹 fence、命令重放和 pending/unknown 恢复；文件和数据库的具体
//! 持久化仍分别由外层适配器实现。文件替换完成而账本提交失败时，服务优先恢复旧文档，
//! 无法证明恢复结果才把命令锁存为 `unknown`。

use std::sync::Arc;

use uuid::Uuid;

use crate::{
    AdminRuntimeAuditRecord, AdminRuntimeCommandState, AdminRuntimeCommandStore,
    AdminRuntimeConfigError, AdminRuntimeConfigPort, AdminRuntimeDocument, AdminRuntimeFileStore,
    AdminRuntimeSnapshot, AdminRuntimeStoredCommand, AdminRuntimeUpdateCommand, ApplicationError,
    ApplicationFuture, Clock,
};

/// Admin runtime 配置应用服务。
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
        let _ = self;
        document.validate().map_err(ApplicationError::from)?;
        Ok(AdminRuntimeSnapshot {
            fingerprint: document.fingerprint(),
            document,
            restart_required: false,
        })
    }

    /// 在 Admin surface 对外就绪前恢复尚未收口的 runtime 命令。
    ///
    /// 启动装配层负责决定哪些结果可以带着告警继续运行；服务层只负责执行与更新流程
    /// 相同的恢复逻辑，避免启动恢复和用户重试各自维护一套状态判断。
    pub async fn recover(&self) -> Result<(), ApplicationError> {
        self.recover_unresolved().await
    }

    /// 执行一次带 durable intent 的 runtime 更新。
    pub async fn update(
        &self,
        command: AdminRuntimeUpdateCommand,
    ) -> Result<AdminRuntimeSnapshot, ApplicationError> {
        command.validate().map_err(ApplicationError::from)?;
        self.recover_unresolved().await?;

        if let Some(stored) = self.commands.find(command.command_id).await? {
            stored.validate().map_err(ApplicationError::from)?;
            if !stored.matches_request(command.request_hash) {
                return Err(AdminRuntimeConfigError::CommandConflict.into());
            }
            return self.finish_existing(stored).await;
        }

        let current = self.files.current().await?;
        current.validate().map_err(ApplicationError::from)?;
        if current.fingerprint != command.expected_fingerprint {
            return Err(AdminRuntimeConfigError::StaleFingerprint.into());
        }

        let now = self.clock.now();
        let stored = self
            .commands
            .insert_pending(AdminRuntimeStoredCommand {
                command_id: command.command_id,
                audit_id: Uuid::new_v4(),
                digest_version: crate::ADMIN_RUNTIME_DIGEST_VERSION,
                request_hash: command.request_hash,
                expected_fingerprint: command.expected_fingerprint,
                old_fingerprint: current.fingerprint,
                new_fingerprint: command.document.fingerprint(),
                candidate_document: command.document,
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
        self.finish_pending(stored).await
    }

    async fn recover_unresolved(&self) -> Result<(), ApplicationError> {
        let rows = self.commands.list_unresolved().await?;
        if rows.len() > 1 {
            return Err(AdminRuntimeConfigError::StoredResultCorrupt.into());
        }
        let Some(row) = rows.into_iter().next() else {
            return Ok(());
        };
        row.validate().map_err(ApplicationError::from)?;
        let current = self.files.current().await?;
        current.validate().map_err(ApplicationError::from)?;
        if current.fingerprint == row.new_fingerprint {
            self.apply_verified(row).await.map(|_| ())
        } else if current.fingerprint == row.old_fingerprint {
            if row.state == AdminRuntimeCommandState::Unknown {
                let reopened = self
                    .commands
                    .reopen(row.command_id, row.request_hash, self.clock.now())
                    .await?;
                self.finish_pending(reopened).await.map(|_| ())
            } else {
                // durable intent 已记录且旧文件仍可证明；继续同一命令，不能永久占用唯一 pending 槽。
                self.finish_pending(row).await.map(|_| ())
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
        stored: AdminRuntimeStoredCommand,
    ) -> Result<AdminRuntimeSnapshot, ApplicationError> {
        match stored.state {
            AdminRuntimeCommandState::Applied => self.finish_applied(stored),
            AdminRuntimeCommandState::Pending => self.finish_pending(stored).await,
            AdminRuntimeCommandState::Unknown => {
                Err(AdminRuntimeConfigError::ReconciliationRequired.into())
            }
        }
    }

    async fn finish_pending(
        &self,
        stored: AdminRuntimeStoredCommand,
    ) -> Result<AdminRuntimeSnapshot, ApplicationError> {
        if stored.state != AdminRuntimeCommandState::Pending {
            return Err(AdminRuntimeConfigError::StoredResultCorrupt.into());
        }
        let current = self.files.current().await?;
        current.validate().map_err(ApplicationError::from)?;
        if current.fingerprint == stored.new_fingerprint {
            return self.apply_verified(stored).await;
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
            .replace(stored.old_fingerprint.clone(), stored.candidate_document.clone())
            .await?;
        if replacement.old_fingerprint != stored.old_fingerprint
            || replacement.new_fingerprint != stored.new_fingerprint
        {
            return self.mark_unknown(stored, "result_corrupt").await;
        }

        match self.apply_row(&stored).await {
            Ok(applied) => self.finish_applied(applied),
            Err(error) => {
                if self.files.restore(replacement).await.is_ok() {
                    Err(error)
                } else {
                    let _ = self
                        .commands
                        .mark_unknown(
                            stored.command_id,
                            stored.request_hash,
                            "replace_unknown".to_owned(),
                            self.clock.now(),
                        )
                        .await;
                    Err(AdminRuntimeConfigError::OutcomeUnknown.into())
                }
            }
        }
    }

    async fn apply_verified(
        &self,
        stored: AdminRuntimeStoredCommand,
    ) -> Result<AdminRuntimeSnapshot, ApplicationError> {
        let current = self.files.current().await?;
        current.validate().map_err(ApplicationError::from)?;
        if current.fingerprint != stored.new_fingerprint
            || stored.candidate_document.fingerprint() != stored.new_fingerprint
        {
            return Err(AdminRuntimeConfigError::StoredResultCorrupt.into());
        }
        let applied = self.apply_row(&stored).await?;
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
