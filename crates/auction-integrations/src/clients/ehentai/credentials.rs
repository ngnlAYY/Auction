//! 单个 E-Hentai Cookie、失效回写与操作会话。

use auction_application::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use std::sync::atomic::{AtomicBool, Ordering};

use super::*;
use auction_application::AdminCredentialWorld;

/// 经过边界校验的 E-Hentai Cookie；克隆共享明文缓冲和不可逆撤销状态。
#[derive(Clone)]
pub struct EhentaiCookie(Arc<EhentaiCookieValue>);

struct EhentaiCookieValue {
    value: String,
    revoked: AtomicBool,
}

impl EhentaiCookie {
    /// 从密钥管理器刚解出的字节建立 Cookie，并拒绝 header 注入字符。
    pub fn from_bytes(value: &[u8]) -> Result<Self, EhentaiError> {
        if value.is_empty()
            || value.len() > MAXIMUM_COOKIE_BYTES
            || value.iter().any(|byte| !(0x20..=0x7e).contains(byte))
        {
            return Err(EhentaiError::credential(EhentaiOperation::CreateTopic));
        }
        let value = String::from_utf8(value.to_vec())
            .map_err(|_| EhentaiError::credential(EhentaiOperation::CreateTopic))?;
        Ok(Self(Arc::new(EhentaiCookieValue { value, revoked: AtomicBool::new(false) })))
    }

    /// 返回仅供建立单次 HTTP header 使用的 Cookie 文本。
    pub(super) fn as_str(&self) -> &str {
        &self.0.value
    }

    pub(super) fn is_active(&self) -> bool {
        !self.0.revoked.load(Ordering::Acquire)
    }

    fn revoke(&self) {
        self.0.revoked.store(true, Ordering::Release);
    }
}

impl PartialEq for EhentaiCookie {
    fn eq(&self, other: &Self) -> bool {
        self.0.value == other.0.value
    }
}

impl Eq for EhentaiCookie {}

impl Drop for EhentaiCookieValue {
    fn drop(&mut self) {
        let mut bytes = std::mem::take(&mut self.value).into_bytes();
        bytes.fill(0);
    }
}

impl fmt::Debug for EhentaiCookie {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EhentaiCookie(REDACTED)")
    }
}

#[derive(Clone)]
struct EhentaiCredentialCandidate {
    key: Uuid,
    persistent_id: Option<Uuid>,
    world: AdminCredentialWorld,
    cookie: EhentaiCookie,
}

/// 单个论坛凭据的受控集合。
#[derive(Clone)]
pub struct EhentaiCredentialSet {
    candidates: Arc<StdMutex<Vec<EhentaiCredentialCandidate>>>,
    invalidator: Option<Arc<dyn AdminCredentialRuntimeWriter>>,
    ownership: Option<Arc<dyn AdminCredentialRuntimeOwnership>>,
}

impl EhentaiCredentialSet {
    /// 从解密后的单个 Cookie 建立凭据集合；空集合只允许数据库 degraded 路径使用。
    pub fn from_cookies(cookies: Vec<EhentaiCookie>) -> Result<Self, EhentaiError> {
        Self::from_candidates(
            cookies
                .into_iter()
                .map(|cookie| EhentaiCredentialCandidate {
                    key: Uuid::new_v4(),
                    persistent_id: None,
                    world: AdminCredentialWorld::Shared,
                    cookie,
                })
                .collect(),
            None,
            None,
            false,
        )
    }

    /// 从数据库解密后的单个 Cookie 及实际 scope 建立可回写失效状态的集合。
    pub fn from_runtime_cookies(
        cookies: Vec<(Uuid, String, EhentaiCookie)>,
        invalidator: Arc<dyn AdminCredentialRuntimeWriter>,
    ) -> Result<Self, EhentaiError> {
        Self::from_candidates(
            cookies
                .into_iter()
                .map(|(persistent_id, scope, cookie)| {
                    Ok(EhentaiCredentialCandidate {
                        key: Uuid::new_v4(),
                        persistent_id: Some(persistent_id),
                        world: credential_world(&scope)?,
                        cookie,
                    })
                })
                .collect::<Result<Vec<_>, EhentaiError>>()?,
            Some(invalidator),
            None,
            true,
        )
    }

    /// 保留数据库实际 scope，并由当前 Worker 独占单 Cookie；配置选择值不能扩大授权范围。
    pub fn from_owned_runtime_cookies(
        cookies: Vec<(Uuid, String, EhentaiCookie)>,
        invalidator: Arc<dyn AdminCredentialRuntimeWriter>,
        ownership: Arc<dyn AdminCredentialRuntimeOwnership>,
    ) -> Result<Self, EhentaiError> {
        Self::from_candidates(
            cookies
                .into_iter()
                .map(|(persistent_id, scope, cookie)| {
                    Ok(EhentaiCredentialCandidate {
                        key: Uuid::new_v4(),
                        persistent_id: Some(persistent_id),
                        world: credential_world(&scope)?,
                        cookie,
                    })
                })
                .collect::<Result<Vec<_>, EhentaiError>>()?,
            Some(invalidator),
            Some(ownership),
            false,
        )
    }

    /// 从本地单 Cookie 建立由当前 Worker 独占的集合。
    pub fn from_owned_cookie(
        cookie: EhentaiCookie,
        world: AdminCredentialWorld,
        ownership: Arc<dyn AdminCredentialRuntimeOwnership>,
    ) -> Result<Self, EhentaiError> {
        Self::from_candidates(
            vec![EhentaiCredentialCandidate {
                key: Uuid::new_v4(),
                persistent_id: None,
                world,
                cookie,
            }],
            None,
            Some(ownership),
            false,
        )
    }

    fn from_candidates(
        candidates: Vec<EhentaiCredentialCandidate>,
        invalidator: Option<Arc<dyn AdminCredentialRuntimeWriter>>,
        ownership: Option<Arc<dyn AdminCredentialRuntimeOwnership>>,
        allow_empty: bool,
    ) -> Result<Self, EhentaiError> {
        if (!allow_empty && candidates.is_empty())
            || candidates.len() > 1
            || candidates
                .iter()
                .any(|candidate| candidate.persistent_id.is_some_and(|id| id.is_nil()))
            || candidates.iter().any(|candidate| candidate.persistent_id.is_some())
                && invalidator.is_none()
        {
            return Err(EhentaiError::credential(EhentaiOperation::CreateTopic));
        }
        if ownership.is_some() && candidates.is_empty() {
            return Err(EhentaiError::credential(EhentaiOperation::CreateTopic));
        }
        Ok(Self { candidates: Arc::new(StdMutex::new(candidates)), invalidator, ownership })
    }

    pub(crate) fn ownership(&self) -> Option<Arc<dyn AdminCredentialRuntimeOwnership>> {
        self.ownership.clone()
    }

    fn ownership_valid(&self) -> bool {
        self.ownership.as_ref().is_none_or(|ownership| ownership.is_valid())
    }

    /// 只有实际 scope 覆盖目标世界且 owner 有效时，调度和网关才允许使用此 Cookie。
    pub(crate) fn available(&self, world: SettlementWorld) -> bool {
        self.ownership_valid()
            && self.candidates.lock().is_ok_and(|candidates| {
                candidates.iter().any(|candidate| {
                    candidate.cookie.is_active() && permits_world(candidate.world, world)
                })
            })
    }

    pub(super) fn begin(
        &self,
        world: Option<SettlementWorld>,
    ) -> Result<EhentaiCredentialSession, EhentaiError> {
        if !self.ownership_valid() {
            return Err(EhentaiError::credential(EhentaiOperation::FetchTopic));
        }
        let candidate = self
            .candidates
            .lock()
            .map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "forum_credentials",
                    "lock_candidates",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                EhentaiError::credential(EhentaiOperation::FetchTopic)
            })?
            .iter()
            .find(|candidate| {
                candidate.cookie.is_active()
                    && world.is_none_or(|world| permits_world(candidate.world, world))
            })
            .cloned()
            .ok_or_else(|| EhentaiError::credential(EhentaiOperation::FetchTopic))?;
        Ok(EhentaiCredentialSession { set: self.clone(), current: candidate })
    }
}

impl fmt::Debug for EhentaiCredentialSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EhentaiCredentialSet")
            .field(
                "candidate_count",
                &self.candidates.lock().map(|candidates| candidates.len()).unwrap_or(0),
            )
            .finish()
    }
}

/// 一次论坛逻辑操作的稳定凭据选择。
pub(super) struct EhentaiCredentialSession {
    set: EhentaiCredentialSet,
    current: EhentaiCredentialCandidate,
}

impl EhentaiCredentialSession {
    pub(super) fn cookie(&self) -> &EhentaiCookie {
        &self.current.cookie
    }

    pub(super) async fn invalidate_current(&mut self) -> Result<(), EhentaiError> {
        let current = &self.current;
        // 先同步撤销已准备的 PM、分页和发帖会话；失效回写取消或失败不能复活这些克隆。
        current.cookie.revoke();
        {
            let mut candidates = self.set.candidates.lock().map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "forum_credentials",
                    "lock_revocation",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                EhentaiError::credential(EhentaiOperation::FetchTopic)
            })?;
            candidates.retain(|candidate| candidate.key != current.key);
        }
        if let (Some(persistent_id), Some(invalidator)) =
            (current.persistent_id, self.set.invalidator.as_ref())
        {
            invalidator.mark_invalid(persistent_id, Utc::now()).await.map_err(
                |diagnostic_cause| {
                    safe_diagnostics::warn(
                        "forum_credentials",
                        "persist_revocation",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                    EhentaiError::credential(EhentaiOperation::FetchTopic)
                },
            )?;
        }
        Ok(())
    }
}

fn credential_world(scope: &str) -> Result<AdminCredentialWorld, EhentaiError> {
    scope
        .strip_prefix("ehentai:")
        .and_then(AdminCredentialWorld::parse)
        .ok_or_else(|| EhentaiError::credential(EhentaiOperation::FetchTopic))
}

fn permits_world(scope: AdminCredentialWorld, world: SettlementWorld) -> bool {
    matches!(
        (scope, world),
        (AdminCredentialWorld::Shared, _)
            | (AdminCredentialWorld::Main, SettlementWorld::Main)
            | (AdminCredentialWorld::Isekai, SettlementWorld::Isekai)
    )
}
