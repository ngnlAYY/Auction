//! 一次逻辑操作的凭据会话：持有稳定选择与操作锁，提交前复核凭据仍是当前有效版本。

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};
use auction_time::Timestamp;
use uuid::Uuid;

use super::super::operation_error;
use super::{MoogleMailCredentialCandidate, MoogleMailCredentialSet};
use crate::clients::mooglemail::{
    MoogleMailCookie, MoogleMailError, MoogleMailErrorCategory, MoogleMailOperation,
    MoogleMailWorld,
};
use crate::clients::operation_lock::OperationLockGuard;
use crate::clients::owner_probe::probe_owner;
use crate::safe_diagnostics;

/// 一次逻辑 MoogleMail 操作的稳定凭据选择。
pub(crate) struct MoogleMailCredentialSession {
    pub(super) set: MoogleMailCredentialSet,
    pub(super) world: MoogleMailWorld,
    pub(super) operation: MoogleMailOperation,
    pub(super) current: MoogleMailCredentialCandidate,
    pub(super) operation_guard: Option<OperationLockGuard>,
}

impl MoogleMailCredentialSession {
    /// 普通观测的持久凭据身份；只用于匹配冻结发送记录，不建立资金/库存授权。
    pub(crate) fn observation_credential_binding(
        &self,
    ) -> Option<auction_application::delivery::DeliverySendCredentialBinding> {
        if !self.set.candidate_is_active(self.current.key, self.world, self.operation).ok()? {
            return None;
        }
        let runtime = self.set.ownership.as_ref()?.mail_runtime_binding()?;
        let binding = auction_application::delivery::DeliverySendCredentialBinding {
            credential_id: self.current.persistent_id?,
            credential_version: self.current.state_version?,
            runtime_generation: runtime.generation,
        };
        binding.validate().then_some(binding)
    }

    /// 网络调用前后以及复用观测后均检查原凭据和 owner；失效不偷偷重建 session。
    pub(crate) async fn check_current(&self) -> Result<(), MoogleMailError> {
        let unavailable = || {
            operation_error(
                self.operation,
                self.world,
                MoogleMailErrorCategory::CredentialUnavailable,
            )
        };
        if !self.set.candidate_is_active(self.current.key, self.world, self.operation)? {
            return Err(unavailable());
        }
        if let Some(owner) = &self.set.ownership {
            // 探测在独立任务中完成：调用方的外层预算到期只放弃等待，不撤销 owner。
            probe_owner(owner).await.map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_credentials",
                    diagnostic_cause.phase("check_owner_timeout", "check_owner"),
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(self.world.as_str()),
                        ..Default::default()
                    },
                );
                unavailable()
            })?;
        }
        if !self.set.candidate_is_active(self.current.key, self.world, self.operation)? {
            return Err(unavailable());
        }
        Ok(())
    }

    /// 本次取得独占 guard 时的派发世界；跨世界读取仍另验 scope。
    pub(crate) fn internal_world(&self) -> MoogleMailWorld {
        self.world
    }

    /// 返回当前 active 数据库身份和持久 fence；配置 Cookie 无稳定 ID，明确不支持。
    pub(crate) fn internal_credential_binding(
        &self,
        world: MoogleMailWorld,
    ) -> Result<
        (Uuid, i64, auction_application::settlement::MailRuntimeBinding, Option<&str>),
        auction_application::settlement::InternalEvidenceError,
    > {
        use auction_application::settlement::InternalEvidenceError as Error;
        if self.operation_guard.is_none() {
            return Err(Error::Unsupported);
        }
        if !self.set.candidate_is_active(self.current.key, world, self.operation).map_err(
            safe_diagnostics::warn_map(
                "mooglemail_credentials",
                "check_internal_candidate",
                || Error::Unavailable,
            ),
        )? {
            return Err(Error::Revoked);
        }
        let id = self.current.persistent_id.ok_or(Error::Unsupported)?;
        let version =
            self.current.state_version.filter(|value| *value > 0).ok_or(Error::Unsupported)?;
        let owner = self.set.ownership.as_ref().ok_or(Error::Unsupported)?;
        let binding = owner
            .mail_runtime_binding()
            .filter(|value| value.generation > 0)
            .ok_or(Error::Unsupported)?;
        Ok((id, version, binding, self.current.player_name.as_deref()))
    }

    /// 同一绝对 deadline 限制等待 owner 探测；失效后不重建 session 或换 Cookie。
    /// deadline 先到只返回 `Expired`：探测在独立任务中继续完成，证据预算耗尽不撤销 owner。
    pub(crate) async fn check_internal_current(
        &self,
        deadline: std::time::Instant,
    ) -> Result<(), auction_application::settlement::InternalEvidenceError> {
        use auction_application::settlement::InternalEvidenceError as Error;
        if std::time::Instant::now() >= deadline {
            return Err(Error::Expired);
        }
        self.internal_credential_binding(self.world)?;
        let owner = self.set.ownership.as_ref().ok_or(Error::Unsupported)?;
        tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), probe_owner(owner))
            .await
            .map_err(safe_diagnostics::warn_map(
                "mooglemail_credentials",
                "check_internal_owner_deadline",
                || Error::Expired,
            ))?
            .map_err(|diagnostic_cause| {
                // 探测自身超时或失败都意味着 owner 已失效，不能再授权内部证据。
                safe_diagnostics::warn(
                    "mooglemail_credentials",
                    diagnostic_cause.phase("check_internal_owner_timeout", "check_internal_owner"),
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                Error::Revoked
            })?;
        if std::time::Instant::now() >= deadline {
            return Err(Error::Expired);
        }
        self.internal_credential_binding(self.world)?;
        Ok(())
    }

    /// 未知内部提交结果同步撤销全部后续邮件操作；不影响论坛 owner。
    pub(crate) fn revoke_internal_owner(&self) {
        if let Some(owner) = &self.set.ownership {
            owner.revoke_internal_use();
        }
        self.current.cookie.revoke();
        if let Ok(mut candidates) = self.set.candidates.lock() {
            candidates.clear();
        }
    }

    /// 返回当前 Cookie；调用方只把它交给同一逻辑操作的受限 HTTP 方法。
    pub(crate) fn cookie(&self) -> &MoogleMailCookie {
        &self.current.cookie
    }

    /// 内部只读流程已经得到明确 MM 登录失效；仅回写当前 MM 凭据，不触及论坛凭据。
    pub(crate) async fn invalidate_internal_current(&self) -> Result<(), MoogleMailError> {
        self.revoke_internal_owner();
        self.invalidate_selected().await
    }

    /// 远端确认登录失效后回写状态并从当前进程池移除当前候选。
    pub(crate) async fn invalidate_current(&self) -> Result<(), MoogleMailError> {
        self.invalidate_selected().await
    }

    async fn invalidate_selected(&self) -> Result<(), MoogleMailError> {
        let current = &self.current;
        current.cookie.revoke();
        {
            let mut candidates = self.set.candidates.lock().map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_credentials",
                    "lock_revocation",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(self.world.as_str()),
                        ..Default::default()
                    },
                );

                operation_error(
                    self.operation,
                    self.world,
                    MoogleMailErrorCategory::CredentialUnavailable,
                )
            })?;
            candidates.retain(|candidate| candidate.key != current.key);
        }
        if let (Some(persistent_id), Some(invalidator)) =
            (current.persistent_id, self.set.invalidator.as_ref())
        {
            invalidator.mark_invalid(persistent_id, Timestamp::now()).await.map_err(
                |diagnostic_cause| {
                    safe_diagnostics::warn(
                        "mooglemail_credentials",
                        "persist_revocation",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext {
                            world: diagnostic_world(self.world.as_str()),
                            ..Default::default()
                        },
                    );

                    operation_error(
                        self.operation,
                        self.world,
                        MoogleMailErrorCategory::CredentialUnavailable,
                    )
                },
            )?;
        }
        Ok(())
    }
}
