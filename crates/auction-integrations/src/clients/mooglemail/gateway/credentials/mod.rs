//! MoogleMail 单 Cookie 普通读会话与资产操作的互斥隔离。

use auction_application::core::diagnostics::DiagnosticContext;

use crate::clients::metrics::MailLockWaitGuard;
use crate::safe_diagnostics;

use std::sync::{Arc, Mutex};

use auction_application::admin::{AdminCredentialRuntimeOwnership, AdminCredentialRuntimeWriter};
use auction_application::mail::GateClass;
use uuid::Uuid;

use super::*;

mod session;

pub(crate) use session::MoogleMailCredentialSession;

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
    external_metrics: Arc<dyn auction_application::external_metrics::ExternalMetricsObserver>,
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
                        !auction_application::admin::valid_mail_player_name(name)
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
        // 只有数据库来源同时具备持久 ID 与版本；检查点作用域不得使用进程内观测 UUID。
        for candidate in &candidates {
            if let (Some(id), Some(version)) = (candidate.persistent_id, candidate.state_version) {
                candidate.cookie.bind_persisted_credential(id, version);
            }
        }
        Ok(Self {
            candidates: Arc::new(Mutex::new(candidates)),
            invalidator,
            ownership,
            external_metrics: auction_application::external_metrics::noop_external_metrics_observer(
            ),
        })
    }

    /// 注入排他锁等待指标；指标失败不改变锁顺序或凭据选择。
    pub fn with_external_metrics(
        mut self,
        observer: Arc<dyn auction_application::external_metrics::ExternalMetricsObserver>,
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
            auction_application::external_metrics::MailLock::Exclusive,
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

impl std::fmt::Debug for MoogleMailCredentialSet {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("MoogleMailCredentialSet")
            .field(&format_args!(
                "[REDACTED; {}]",
                self.candidates.lock().map_or(0, |candidates| candidates.len())
            ))
            .finish()
    }
}
