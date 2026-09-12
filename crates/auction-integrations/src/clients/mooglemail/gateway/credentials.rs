//! MoogleMail 单 Cookie 会话和一次操作内的写入隔离。

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
    world: Option<MoogleMailWorld>,
    cookie: MoogleMailCookie,
    operation_lock: Arc<AsyncMutex<()>>,
}

/// 单个运行时 MoogleMail Cookie 的受控集合。
///
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

    fn from_candidates(
        candidates: Vec<MoogleMailCredentialCandidate>,
        invalidator: Option<Arc<dyn AdminCredentialRuntimeWriter>>,
        ownership: Option<Arc<dyn AdminCredentialRuntimeOwnership>>,
        allow_empty: bool,
    ) -> Result<Self, MoogleMailError> {
        if (!allow_empty && candidates.is_empty())
            || candidates.len() > 1
            || candidates
                .iter()
                .any(|candidate| candidate.persistent_id.is_some_and(|id| id.is_nil()))
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

    /// 将同一 owner 传给每物理请求的 HTTP 边界，不重新获取连接或凭据。
    pub(crate) fn ownership(&self) -> Option<Arc<dyn AdminCredentialRuntimeOwnership>> {
        self.ownership.clone()
    }

    fn ownership_valid(&self) -> bool {
        self.ownership.as_ref().is_none_or(|ownership| ownership.is_valid())
    }

    pub(crate) fn begin(
        &self,
        world: MoogleMailWorld,
    ) -> Result<MoogleMailCredentialSession, MoogleMailError> {
        let operation = MoogleMailOperation::Inbox;
        let mut tried = HashSet::new();
        let candidate = self.next_candidate(world, operation, &tried)?;
        tried.insert(candidate.key);
        Ok(MoogleMailCredentialSession {
            set: self.clone(),
            world,
            operation,
            current: candidate.clone(),
            tried,
            operation_guard: None,
        })
    }

    /// 为会修改远端状态的完整逻辑操作取得凭据级独占 guard。
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
    /// 返回当前 Cookie；调用方只把它交给同一逻辑操作的受限 HTTP 方法。
    pub(crate) fn cookie(&self) -> &MoogleMailCookie {
        &self.current.cookie
    }

    /// 远端确认登录失效后回写状态并从当前进程池移除当前候选。
    pub(crate) async fn invalidate_current(&mut self) -> Result<(), MoogleMailError> {
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

    /// 兼容调用边界：当前单 Cookie 模型没有后备凭据，失效后会返回不可用。
    pub(crate) fn advance(&mut self) -> Result<(), MoogleMailError> {
        debug_assert!(self.operation_guard.is_none());
        let candidate = self.set.next_candidate(self.world, self.operation, &self.tried)?;
        self.tried.insert(candidate.key);
        self.current = candidate;
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
