//! 单个 E-Hentai Cookie、失效回写与操作会话。

use super::*;

/// 经过边界校验的 E-Hentai Cookie。
#[derive(Clone, PartialEq, Eq)]
pub struct EhentaiCookie(pub(super) String);

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
        Ok(Self(value))
    }

    /// 返回仅供建立单次 HTTP header 使用的 Cookie 文本。
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

impl Drop for EhentaiCookie {
    fn drop(&mut self) {
        let mut bytes = std::mem::take(&mut self.0).into_bytes();
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
                    cookie,
                })
                .collect(),
            None,
            None,
            false,
        )
    }

    /// 从数据库解密后的单个 Cookie 建立可回写失效状态的集合。
    pub fn from_runtime_cookies(
        cookies: Vec<(Uuid, EhentaiCookie)>,
        invalidator: Arc<dyn AdminCredentialRuntimeWriter>,
    ) -> Result<Self, EhentaiError> {
        Self::from_candidates(
            cookies
                .into_iter()
                .map(|(persistent_id, cookie)| EhentaiCredentialCandidate {
                    key: Uuid::new_v4(),
                    persistent_id: Some(persistent_id),
                    cookie,
                })
                .collect(),
            Some(invalidator),
            None,
            true,
        )
    }

    /// 从数据库解密后的单 Cookie 建立由当前 Worker 独占且可回写失效状态的集合。
    pub fn from_owned_runtime_cookies(
        cookies: Vec<(Uuid, EhentaiCookie)>,
        invalidator: Arc<dyn AdminCredentialRuntimeWriter>,
        ownership: Arc<dyn AdminCredentialRuntimeOwnership>,
    ) -> Result<Self, EhentaiError> {
        Self::from_candidates(
            cookies
                .into_iter()
                .map(|(persistent_id, cookie)| EhentaiCredentialCandidate {
                    key: Uuid::new_v4(),
                    persistent_id: Some(persistent_id),
                    cookie,
                })
                .collect(),
            Some(invalidator),
            Some(ownership),
            false,
        )
    }

    /// 从本地单 Cookie 建立由当前 Worker 独占的集合。
    pub fn from_owned_cookie(
        cookie: EhentaiCookie,
        ownership: Arc<dyn AdminCredentialRuntimeOwnership>,
    ) -> Result<Self, EhentaiError> {
        Self::from_candidates(
            vec![EhentaiCredentialCandidate { key: Uuid::new_v4(), persistent_id: None, cookie }],
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

    pub fn from_secret(secret: &[u8]) -> Result<Self, EhentaiError> {
        Self::from_cookies(vec![EhentaiCookie::from_bytes(secret)?])
    }

    pub(crate) fn ownership(&self) -> Option<Arc<dyn AdminCredentialRuntimeOwnership>> {
        self.ownership.clone()
    }

    fn ownership_valid(&self) -> bool {
        self.ownership.as_ref().is_none_or(|ownership| ownership.is_valid())
    }

    /// 返回候选集是否为空；已失去 owner 的集合不再提供凭据。
    pub fn is_empty(&self) -> bool {
        !self.ownership_valid()
            || self.candidates.lock().map(|candidates| candidates.is_empty()).unwrap_or(true)
    }

    pub(super) fn begin(&self) -> Result<EhentaiCredentialSession, EhentaiError> {
        if !self.ownership_valid() {
            return Err(EhentaiError::credential(EhentaiOperation::FetchTopic));
        }
        let candidate = self
            .candidates
            .lock()
            .map_err(|_| EhentaiError::credential(EhentaiOperation::FetchTopic))?
            .first()
            .cloned()
            .ok_or_else(|| EhentaiError::credential(EhentaiOperation::FetchTopic))?;
        Ok(EhentaiCredentialSession {
            set: self.clone(),
            tried: HashSet::from([candidate.key]),
            current: candidate,
        })
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
    tried: HashSet<Uuid>,
}

impl EhentaiCredentialSession {
    pub(super) fn cookie(&self) -> &EhentaiCookie {
        &self.current.cookie
    }

    pub(super) async fn invalidate_current(&mut self) -> Result<(), EhentaiError> {
        let current = &self.current;
        {
            let mut candidates = self
                .set
                .candidates
                .lock()
                .map_err(|_| EhentaiError::credential(EhentaiOperation::FetchTopic))?;
            candidates.retain(|candidate| candidate.key != current.key);
        }
        if let (Some(persistent_id), Some(invalidator)) =
            (current.persistent_id, self.set.invalidator.as_ref())
        {
            invalidator
                .mark_invalid(persistent_id, Utc::now())
                .await
                .map_err(|_| EhentaiError::credential(EhentaiOperation::FetchTopic))?;
        }
        Ok(())
    }

    pub(super) fn advance(&mut self) -> Result<(), EhentaiError> {
        if !self.set.ownership_valid() {
            return Err(EhentaiError::credential(EhentaiOperation::FetchTopic));
        }
        let candidate = self
            .set
            .candidates
            .lock()
            .map_err(|_| EhentaiError::credential(EhentaiOperation::FetchTopic))?
            .iter()
            .find(|candidate| !self.tried.contains(&candidate.key))
            .cloned()
            .ok_or_else(|| EhentaiError::credential(EhentaiOperation::FetchTopic))?;
        self.tried.insert(candidate.key);
        self.current = candidate;
        Ok(())
    }
}
