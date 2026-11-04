//! MoogleMail 单 Cookie 普通读会话与资产操作的互斥隔离。

use auction_application::diagnostics::{DiagnosticContext, diagnostic_world};

use crate::clients::metrics::MailLockWaitGuard;
use crate::diagnostics as safe_diagnostics;

use std::sync::{Arc, Mutex};

use auction_application::{
    AdminCredentialRuntimeOwnership, AdminCredentialRuntimeWriter, GateClass,
};
use chrono::Utc;
use uuid::Uuid;

use super::*;

#[derive(Clone)]
struct MoogleMailCredentialCandidate {
    key: Uuid,
    persistent_id: Option<Uuid>,
    state_version: Option<i64>,
    player_name: Option<String>,
    world: Option<MoogleMailWorld>,
    cookie: MoogleMailCookie,
    operation_lock: Arc<OperationLock>,
}

/// 维护的 compose 观察与资产写入共用同一把锁；只有 compose 页面观察属于后台维护，
/// 发送与资产动作属于前台写入，不能被维护观察无限推迟。
fn operation_lock_class(operation: MoogleMailOperation) -> GateClass {
    match operation {
        MoogleMailOperation::Compose => GateClass::Background,
        _ => GateClass::Foreground,
    }
}

/// 数据库凭据的运行时身份元数据；名称仅为预期值，Cookie 的 Debug 已脱敏。
#[derive(Debug)]
pub struct MoogleMailRuntimeCredential {
    pub credential_id: Uuid,
    pub state_version: i64,
    pub world: Option<MoogleMailWorld>,
    pub player_name: Option<String>,
    pub cookie: MoogleMailCookie,
}

/// 单个运行时 MoogleMail Cookie 的受控集合。
/// 集合只保存脱敏 Cookie 值对象，不实现 `Serialize`，也不提供把值写入日志或 DTO 的接口。
/// 数据库来源附带持久 ID，远端确认登录失效后会通过窄端口回写 `invalid`，并从当前进程移除。
#[derive(Clone)]
pub struct MoogleMailCredentialSet {
    candidates: Arc<Mutex<Vec<MoogleMailCredentialCandidate>>>,
    invalidator: Option<Arc<dyn AdminCredentialRuntimeWriter>>,
    ownership: Option<Arc<dyn AdminCredentialRuntimeOwnership>>,
    external_metrics: Arc<dyn auction_application::ExternalMetricsObserver>,
}

impl MoogleMailCredentialSet {
    /// 由配置层显式 getter 建立单 Cookie 集合。
    pub fn new(cookie: &str) -> Result<Self, MoogleMailError> {
        Self::from_cookies(vec![MoogleMailCookie::new(cookie)?])
    }

    /// 从本地单 Cookie 建立由当前 Worker 独占的集合。
    pub fn from_owned_cookie(
        cookie: MoogleMailCookie,
        ownership: Arc<dyn AdminCredentialRuntimeOwnership>,
    ) -> Result<Self, MoogleMailError> {
        Self::from_candidates(
            vec![MoogleMailCredentialCandidate {
                key: Uuid::new_v4(),
                persistent_id: None,
                state_version: None,
                player_name: None,
                world: None,
                cookie,
                operation_lock: Arc::new(OperationLock::new()),
            }],
            None,
            Some(ownership),
            false,
        )
    }

    /// 从不带持久 ID 的本地单 Cookie 建立集合。
    pub fn from_cookies(cookies: Vec<MoogleMailCookie>) -> Result<Self, MoogleMailError> {
        Self::from_candidates(
            cookies
                .into_iter()
                .map(|cookie| MoogleMailCredentialCandidate {
                    key: Uuid::new_v4(),
                    persistent_id: None,
                    state_version: None,
                    player_name: None,
                    world: None,
                    cookie,
                    operation_lock: Arc::new(OperationLock::new()),
                })
                .collect(),
            None,
            None,
            false,
        )
    }

    /// 从数据库解密后的单 Cookie 建立可回写失效状态的集合。
    pub fn from_runtime_cookies(
        cookies: Vec<(Uuid, MoogleMailCookie)>,
        invalidator: Arc<dyn AdminCredentialRuntimeWriter>,
    ) -> Result<Self, MoogleMailError> {
        Self::from_candidates(
            cookies
                .into_iter()
                .map(|(persistent_id, cookie)| MoogleMailCredentialCandidate {
                    key: Uuid::new_v4(),
                    persistent_id: Some(persistent_id),
                    state_version: None,
                    player_name: None,
                    world: None,
                    cookie,
                    operation_lock: Arc::new(OperationLock::new()),
                })
                .collect(),
            Some(invalidator),
            None,
            true,
        )
    }

    /// 从带 world scope 的数据库单凭据建立集合；`None` 表示 shared，可服务两个世界。
    pub fn from_runtime_cookies_with_scopes(
        cookies: Vec<(Uuid, Option<MoogleMailWorld>, MoogleMailCookie)>,
        invalidator: Arc<dyn AdminCredentialRuntimeWriter>,
    ) -> Result<Self, MoogleMailError> {
        Self::from_candidates(
            cookies
                .into_iter()
                .map(|(persistent_id, world, cookie)| MoogleMailCredentialCandidate {
                    key: Uuid::new_v4(),
                    persistent_id: Some(persistent_id),
                    state_version: None,
                    player_name: None,
                    world,
                    cookie,
                    operation_lock: Arc::new(OperationLock::new()),
                })
                .collect(),
            Some(invalidator),
            None,
            true,
        )
    }

    /// 从当前凭据版本及预期名称建立集合；不据此创建任何远端身份证明。
    pub fn from_runtime_credentials(
        credentials: Vec<MoogleMailRuntimeCredential>,
        invalidator: Arc<dyn AdminCredentialRuntimeWriter>,
        ownership: Option<Arc<dyn AdminCredentialRuntimeOwnership>>,
    ) -> Result<Self, MoogleMailError> {
        let allow_empty = ownership.is_none();
        Self::from_candidates(
            credentials
                .into_iter()
                .map(|credential| MoogleMailCredentialCandidate {
                    key: Uuid::new_v4(),
                    persistent_id: Some(credential.credential_id),
                    state_version: Some(credential.state_version),
                    player_name: credential.player_name,
                    world: credential.world,
                    cookie: credential.cookie,
                    operation_lock: Arc::new(OperationLock::new()),
                })
                .collect(),
            Some(invalidator),
            ownership,
            allow_empty,
        )
    }

    fn from_candidates(
        candidates: Vec<MoogleMailCredentialCandidate>,
        invalidator: Option<Arc<dyn AdminCredentialRuntimeWriter>>,
        ownership: Option<Arc<dyn AdminCredentialRuntimeOwnership>>,
        allow_empty: bool,
    ) -> Result<Self, MoogleMailError> {
        if (!allow_empty && candidates.is_empty())
            || candidates.len() > 1
            || candidates.iter().any(|candidate| {
                candidate.persistent_id.is_some_and(|id| id.is_nil())
                    || candidate.state_version.is_some_and(|version| version <= 0)
                    || candidate.player_name.as_deref().is_some_and(|name| {
                        !auction_application::valid_mail_player_name(name)
                            || candidate.persistent_id.is_none()
                            || candidate.state_version.is_none()
                    })
            })
        {
            return Err(MoogleMailError::new(
                MoogleMailOperation::Inbox,
                MoogleMailWorld::Main,
                MoogleMailErrorCategory::CredentialUnavailable,
                false,
            ));
        }
        if candidates.iter().any(|candidate| candidate.persistent_id.is_some())
            && invalidator.is_none()
        {
            return Err(MoogleMailError::new(
                MoogleMailOperation::Inbox,
                MoogleMailWorld::Main,
                MoogleMailErrorCategory::CredentialUnavailable,
                false,
            ));
        }
        if ownership.is_some() && candidates.is_empty() {
            return Err(MoogleMailError::new(
                MoogleMailOperation::Inbox,
                MoogleMailWorld::Main,
                MoogleMailErrorCategory::CredentialUnavailable,
                false,
            ));
        }
        Ok(Self {
            candidates: Arc::new(Mutex::new(candidates)),
            invalidator,
            ownership,
            external_metrics: auction_application::noop_external_metrics_observer(),
        })
    }

    /// 注入排他锁等待指标；指标失败不改变锁顺序或凭据选择。
    pub fn with_external_metrics(
        mut self,
        observer: Arc<dyn auction_application::ExternalMetricsObserver>,
    ) -> Self {
        self.external_metrics = observer;
        self
    }

    /// 同 session 复用入口只能使用原集合，防止把另一 owner 的 HTTP client 配到此 Cookie。
    pub(crate) fn owns_session(&self, session: &MoogleMailCredentialSession) -> bool {
        Arc::ptr_eq(&self.candidates, &session.set.candidates)
    }

    /// 将同一 owner 传给每物理请求的 HTTP 边界，不重新获取连接或凭据。
    pub(crate) fn ownership(&self) -> Option<Arc<dyn AdminCredentialRuntimeOwnership>> {
        self.ownership.clone()
    }

    fn ownership_valid(&self) -> bool {
        self.ownership.as_ref().is_none_or(|ownership| ownership.is_valid())
    }

    /// 普通读取只固定当前凭据，不等待 compose/资产操作 guard，也不能授权内部现金证据。
    pub(crate) async fn begin_read(
        &self,
        world: MoogleMailWorld,
        operation: MoogleMailOperation,
    ) -> Result<MoogleMailCredentialSession, MoogleMailError> {
        let current = self.current_candidate(world, operation)?;
        let session = MoogleMailCredentialSession {
            set: self.clone(),
            world,
            operation,
            current,
            operation_guard: None,
        };
        session.check_current().await?;
        Ok(session)
    }

    /// 为 compose、资产动作或内部证据取得独占 guard；普通邮箱读取使用 begin_read。
    pub(crate) async fn begin_exclusive(
        &self,
        world: MoogleMailWorld,
        operation: MoogleMailOperation,
    ) -> Result<MoogleMailCredentialSession, MoogleMailError> {
        let current = self.current_candidate(world, operation)?;
        let lock_wait = MailLockWaitGuard::start(
            Arc::clone(&self.external_metrics),
            world,
            operation,
            auction_application::MailLock::Exclusive,
        );
        let operation_guard =
            Arc::clone(&current.operation_lock).lock(operation_lock_class(operation)).await;
        lock_wait.acquired();
        // 集合最多一条凭据且只会撤销；等待期间失效必须失败，不能重获或换号。
        if !self.candidate_is_active(current.key, world, operation)? {
            return Err(operation_error(
                operation,
                world,
                MoogleMailErrorCategory::CredentialUnavailable,
            ));
        }
        Ok(MoogleMailCredentialSession {
            set: self.clone(),
            world,
            operation,
            current,
            operation_guard: Some(operation_guard),
        })
    }

    fn current_candidate(
        &self,
        world: MoogleMailWorld,
        operation: MoogleMailOperation,
    ) -> Result<MoogleMailCredentialCandidate, MoogleMailError> {
        if !self.ownership_valid() {
            return Err(operation_error(
                operation,
                world,
                MoogleMailErrorCategory::CredentialUnavailable,
            ));
        }
        self.candidates
            .lock()
            .map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_credentials",
                    "lock_candidates",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );

                operation_error(operation, world, MoogleMailErrorCategory::CredentialUnavailable)
            })?
            .iter()
            .find(|candidate| {
                candidate.world.is_none_or(|scope| scope == world) && candidate.cookie.is_active()
            })
            .cloned()
            .ok_or_else(|| {
                operation_error(operation, world, MoogleMailErrorCategory::CredentialUnavailable)
            })
    }

    fn candidate_is_active(
        &self,
        key: Uuid,
        world: MoogleMailWorld,
        operation: MoogleMailOperation,
    ) -> Result<bool, MoogleMailError> {
        if !self.ownership_valid() {
            return Ok(false);
        }
        self.candidates
            .lock()
            .map(|candidates| {
                candidates.iter().any(|candidate| {
                    candidate.key == key
                        && candidate.cookie.is_active()
                        && candidate.world.is_none_or(|scope| scope == world)
                })
            })
            .map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_credentials",
                    "check_candidate",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );

                operation_error(operation, world, MoogleMailErrorCategory::CredentialUnavailable)
            })
    }

    /// 返回当前进程中是否至少有一个能够服务目标 world 的凭据。
    pub(crate) fn available(&self, world: MoogleMailWorld) -> bool {
        self.ownership_valid()
            && self.candidates.lock().is_ok_and(|candidates| {
                candidates.iter().any(|candidate| {
                    candidate.cookie.is_active()
                        && candidate.world.is_none_or(|scope| scope == world)
                })
            })
    }
}

/// 一次逻辑 MoogleMail 操作的稳定凭据选择。
pub(crate) struct MoogleMailCredentialSession {
    set: MoogleMailCredentialSet,
    world: MoogleMailWorld,
    operation: MoogleMailOperation,
    current: MoogleMailCredentialCandidate,
    operation_guard: Option<OperationLockGuard>,
}

impl MoogleMailCredentialSession {
    /// 普通观测的持久凭据身份；只用于匹配冻结发送记录，不建立资金/库存授权。
    pub(crate) fn observation_credential_binding(
        &self,
    ) -> Option<auction_application::DeliverySendCredentialBinding> {
        if !self.set.candidate_is_active(self.current.key, self.world, self.operation).ok()? {
            return None;
        }
        let runtime = self.set.ownership.as_ref()?.mail_runtime_binding()?;
        let binding = auction_application::DeliverySendCredentialBinding {
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
            tokio::time::timeout(std::time::Duration::from_secs(1), owner.check())
                .await
                .map_err(|diagnostic_cause| {
                    safe_diagnostics::warn(
                        "mooglemail_credentials",
                        "check_owner_timeout",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext {
                            world: diagnostic_world(self.world.as_str()),
                            ..Default::default()
                        },
                    );
                    unavailable()
                })?
                .map_err(|diagnostic_cause| {
                    safe_diagnostics::warn(
                        "mooglemail_credentials",
                        "check_owner",
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
        (Uuid, i64, auction_application::MailRuntimeBinding, Option<&str>),
        auction_application::InternalEvidenceError,
    > {
        use auction_application::InternalEvidenceError as Error;
        if self.operation_guard.is_none() {
            return Err(Error::Unsupported);
        }
        if !self.set.candidate_is_active(self.current.key, world, self.operation).map_err(
            |diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_credentials",
                    "check_internal_candidate",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                Error::Unavailable
            },
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

    /// 同一绝对 deadline 覆盖 owner 探测；失效后不重建 session 或换 Cookie。
    pub(crate) async fn check_internal_current(
        &self,
        deadline: std::time::Instant,
    ) -> Result<(), auction_application::InternalEvidenceError> {
        use auction_application::InternalEvidenceError as Error;
        if std::time::Instant::now() >= deadline {
            return Err(Error::Expired);
        }
        self.internal_credential_binding(self.world)?;
        let owner = self.set.ownership.as_ref().ok_or(Error::Unsupported)?;
        tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), owner.check())
            .await
            .map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_credentials",
                    "check_internal_owner_timeout",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                Error::Expired
            })?
            .map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_credentials",
                    "check_internal_owner",
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
    pub(crate) async fn invalidate_current(&mut self) -> Result<(), MoogleMailError> {
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
            invalidator.mark_invalid(persistent_id, Utc::now()).await.map_err(
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

impl std::fmt::Debug for MoogleMailCredentialSet {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("MoogleMailCredentialSet")
            .field(&format_args!(
                "[REDACTED; {}]",
                self.candidates.lock().map(|candidates| candidates.len()).unwrap_or(0)
            ))
            .finish()
    }
}
