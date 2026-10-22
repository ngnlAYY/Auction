//! MoogleMail 单 Cookie 会话和完整逻辑操作的互斥隔离。

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use auction_application::{AdminCredentialRuntimeOwnership, AdminCredentialRuntimeWriter};
use chrono::Utc;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};
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
    operation_lock: Arc<AsyncMutex<()>>,
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
                operation_lock: Arc::new(AsyncMutex::new(())),
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
                    operation_lock: Arc::new(AsyncMutex::new(())),
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
                    operation_lock: Arc::new(AsyncMutex::new(())),
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
                    operation_lock: Arc::new(AsyncMutex::new(())),
                })
                .collect(),
            Some(invalidator),
            None,
            true,
        )
    }

    /// 从带 scope 的数据库单凭据建立由当前 Worker 独占的集合。
    pub fn from_owned_runtime_cookies_with_scopes(
        cookies: Vec<(Uuid, Option<MoogleMailWorld>, MoogleMailCookie)>,
        invalidator: Arc<dyn AdminCredentialRuntimeWriter>,
        ownership: Arc<dyn AdminCredentialRuntimeOwnership>,
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
                    operation_lock: Arc::new(AsyncMutex::new(())),
                })
                .collect(),
            Some(invalidator),
            Some(ownership),
            false,
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
                    operation_lock: Arc::new(AsyncMutex::new(())),
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
        Ok(Self { candidates: Arc::new(Mutex::new(candidates)), invalidator, ownership })
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

    /// 为完整逻辑读写操作取得凭据级独占 guard；已有 session 的内部读取不得重复取得。
    pub(crate) async fn begin_exclusive(
        &self,
        world: MoogleMailWorld,
        operation: MoogleMailOperation,
    ) -> Result<MoogleMailCredentialSession, MoogleMailError> {
        let mut tried = HashSet::new();
        for _ in 0..64 {
            let candidate = self.next_candidate(world, operation, &tried)?;
            tried.insert(candidate.key);
            let operation_guard = Arc::clone(&candidate.operation_lock).lock_owned().await;
            if self.candidate_is_active(candidate.key, world, operation)? {
                return Ok(MoogleMailCredentialSession {
                    set: self.clone(),
                    world,
                    operation,
                    current: candidate,
                    tried,
                    operation_guard: Some(operation_guard),
                });
            }
        }
        Err(operation_error(operation, world, MoogleMailErrorCategory::CredentialUnavailable))
    }

    fn next_candidate(
        &self,
        world: MoogleMailWorld,
        operation: MoogleMailOperation,
        tried: &HashSet<Uuid>,
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
            .map_err(|_| {
                operation_error(operation, world, MoogleMailErrorCategory::CredentialUnavailable)
            })?
            .iter()
            .find(|candidate| {
                candidate.world.is_none_or(|scope| scope == world)
                    && !tried.contains(&candidate.key)
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
                    candidate.key == key && candidate.world.is_none_or(|scope| scope == world)
                })
            })
            .map_err(|_| {
                operation_error(operation, world, MoogleMailErrorCategory::CredentialUnavailable)
            })
    }

    /// 返回当前进程中是否至少有一个能够服务目标 world 的凭据。
    pub(crate) fn available(&self, world: MoogleMailWorld) -> bool {
        self.ownership_valid()
            && self.candidates.lock().is_ok_and(|candidates| {
                candidates
                    .iter()
                    .any(|candidate| candidate.world.is_none_or(|scope| scope == world))
            })
    }
}

/// 一次逻辑 MoogleMail 操作的稳定凭据选择。
pub(crate) struct MoogleMailCredentialSession {
    set: MoogleMailCredentialSet,
    world: MoogleMailWorld,
    operation: MoogleMailOperation,
    current: MoogleMailCredentialCandidate,
    tried: HashSet<Uuid>,
    operation_guard: Option<OwnedMutexGuard<()>>,
}

impl MoogleMailCredentialSession {
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
        if !self
            .set
            .candidate_is_active(self.current.key, world, self.operation)
            .map_err(|_| Error::Unavailable)?
        {
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
        self.internal_credential_binding(self.world)?;
        let owner = self.set.ownership.as_ref().ok_or(Error::Unsupported)?;
        tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), owner.check())
            .await
            .map_err(|_| Error::Expired)?
            .map_err(|_| Error::Revoked)?;
        self.internal_credential_binding(self.world)?;
        Ok(())
    }

    /// 未知内部提交结果同步撤销全部后续邮件操作；不影响论坛 owner。
    pub(crate) fn revoke_internal_owner(&self) {
        if let Some(owner) = &self.set.ownership {
            owner.revoke_internal_use();
        }
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
        let current = self.current.clone();
        {
            let mut candidates = self.set.candidates.lock().map_err(|_| {
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
            invalidator.mark_invalid(persistent_id, Utc::now()).await.map_err(|_| {
                operation_error(
                    self.operation,
                    self.world,
                    MoogleMailErrorCategory::CredentialUnavailable,
                )
            })?;
        }
        Ok(())
    }

    /// 释放当前凭据 guard；当前单 Cookie 失效后不会自动切换到其它 Cookie。
    pub(crate) async fn advance_exclusive(&mut self) -> Result<(), MoogleMailError> {
        self.operation_guard.take();
        for _ in 0..64 {
            let candidate = self.set.next_candidate(self.world, self.operation, &self.tried)?;
            self.tried.insert(candidate.key);
            let operation_guard = Arc::clone(&candidate.operation_lock).lock_owned().await;
            if self.set.candidate_is_active(candidate.key, self.world, self.operation)? {
                self.current = candidate;
                self.operation_guard = Some(operation_guard);
                return Ok(());
            }
        }
        Err(operation_error(
            self.operation,
            self.world,
            MoogleMailErrorCategory::CredentialUnavailable,
        ))
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
