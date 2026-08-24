//! MoogleMail 凭据候选池和一次操作内的 Cookie 轮换。

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use auction_application::AdminCredentialRuntimeWriter;
use chrono::Utc;
use uuid::Uuid;

use super::*;

#[derive(Clone)]
struct MoogleMailCredentialCandidate {
    key: Uuid,
    persistent_id: Option<Uuid>,
    world: Option<MoogleMailWorld>,
    cookie: MoogleMailCookie,
}

/// 两个本地 `.keyenv` 身份或数据库凭据池的受控 Cookie 集合。
///
/// `bot` 对应 `hv-ck-key1`，`test` 对应 `hv-ck-key2`。集合只保存脱敏 Cookie 值对象，不
/// 实现 `Serialize`，也不提供把值写入日志或 DTO 的接口。数据库来源附带持久 ID，远端确认
/// 登录失效后会通过窄端口回写 `invalid`，并从当前进程池移除。
#[derive(Clone)]
pub struct MoogleMailCredentialSet {
    candidates: Arc<Mutex<Vec<MoogleMailCredentialCandidate>>>,
    invalidator: Option<Arc<dyn AdminCredentialRuntimeWriter>>,
}

impl MoogleMailCredentialSet {
    /// 由配置层显式 getter 建立 Bot/测试身份集合。
    pub fn new(bot: &str, test: &str) -> Result<Self, MoogleMailError> {
        Self::from_cookies(vec![MoogleMailCookie::new(bot)?, MoogleMailCookie::new(test)?])
    }

    /// 从不带持久 ID 的本地 Cookie 建立稳定顺序的集合。
    pub fn from_cookies(cookies: Vec<MoogleMailCookie>) -> Result<Self, MoogleMailError> {
        Self::from_candidates(
            cookies
                .into_iter()
                .map(|cookie| MoogleMailCredentialCandidate {
                    key: Uuid::new_v4(),
                    persistent_id: None,
                    world: None,
                    cookie,
                })
                .collect(),
            None,
            false,
        )
    }

    /// 从数据库解密后的 Cookie 建立可回写失效状态的集合。
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
                })
                .collect(),
            Some(invalidator),
            true,
        )
    }

    /// 从带 world scope 的数据库凭据建立候选池；`None` 表示 shared，可服务两个世界。
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
                })
                .collect(),
            Some(invalidator),
            true,
        )
    }

    fn from_candidates(
        candidates: Vec<MoogleMailCredentialCandidate>,
        invalidator: Option<Arc<dyn AdminCredentialRuntimeWriter>>,
        allow_empty: bool,
    ) -> Result<Self, MoogleMailError> {
        if (!allow_empty && candidates.is_empty())
            || candidates.len() > 64
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
        Ok(Self { candidates: Arc::new(Mutex::new(candidates)), invalidator })
    }

    pub(crate) fn begin(
        &self,
        world: MoogleMailWorld,
    ) -> Result<MoogleMailCredentialSession, MoogleMailError> {
        let candidate = self
            .candidates
            .lock()
            .map_err(|_| {
                operation_error(
                    MoogleMailOperation::Inbox,
                    world,
                    MoogleMailErrorCategory::CredentialUnavailable,
                )
            })?
            .iter()
            .find(|candidate| candidate.world.is_none_or(|scope| scope == world))
            .cloned()
            .ok_or_else(|| {
                operation_error(
                    MoogleMailOperation::Inbox,
                    world,
                    MoogleMailErrorCategory::CredentialUnavailable,
                )
            })?;
        Ok(MoogleMailCredentialSession {
            set: self.clone(),
            world,
            current: candidate.clone(),
            tried: HashSet::from([candidate.key]),
        })
    }

    /// 返回当前进程中是否至少有一个能够服务目标 world 的凭据。
    pub(crate) fn available(&self, world: MoogleMailWorld) -> bool {
        self.candidates.lock().is_ok_and(|candidates| {
            candidates.iter().any(|candidate| candidate.world.is_none_or(|scope| scope == world))
        })
    }
}

/// 一次逻辑 MoogleMail 操作的稳定凭据选择。
pub(crate) struct MoogleMailCredentialSession {
    set: MoogleMailCredentialSet,
    world: MoogleMailWorld,
    current: MoogleMailCredentialCandidate,
    tried: HashSet<Uuid>,
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
                    MoogleMailOperation::Inbox,
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
                    MoogleMailOperation::Inbox,
                    self.world,
                    MoogleMailErrorCategory::CredentialUnavailable,
                )
            })?;
        }
        Ok(())
    }

    /// 切换到尚未尝试过的稳定候选。
    pub(crate) fn advance(&mut self) -> Result<(), MoogleMailError> {
        let candidate = self
            .set
            .candidates
            .lock()
            .map_err(|_| {
                operation_error(
                    MoogleMailOperation::Inbox,
                    self.world,
                    MoogleMailErrorCategory::CredentialUnavailable,
                )
            })?
            .iter()
            .find(|candidate| {
                candidate.world.is_none_or(|scope| scope == self.world)
                    && !self.tried.contains(&candidate.key)
            })
            .cloned()
            .ok_or_else(|| {
                operation_error(
                    MoogleMailOperation::Inbox,
                    self.world,
                    MoogleMailErrorCategory::CredentialUnavailable,
                )
            })?;
        self.tried.insert(candidate.key);
        self.current = candidate;
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
