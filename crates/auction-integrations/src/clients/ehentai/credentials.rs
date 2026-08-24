//! E-Hentai Cookie 凭据候选集、失效回写与轮换会话。

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

/// 一组按稳定顺序尝试的论坛凭据。
#[derive(Clone)]
pub struct EhentaiCredentialSet {
    candidates: Arc<StdMutex<Vec<EhentaiCredentialCandidate>>>,
    invalidator: Option<Arc<dyn AdminCredentialRuntimeWriter>>,
}

impl EhentaiCredentialSet {
    /// 从解密后的 Cookie 建立凭据候选集。
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
            false,
        )
    }

    /// 从数据库解密后的 Cookie 建立可回写失效状态的集合。
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
            true,
        )
    }

    fn from_candidates(
        candidates: Vec<EhentaiCredentialCandidate>,
        invalidator: Option<Arc<dyn AdminCredentialRuntimeWriter>>,
        allow_empty: bool,
    ) -> Result<Self, EhentaiError> {
        if (!allow_empty && candidates.is_empty())
            || candidates.len() > 64
            || candidates
                .iter()
                .any(|candidate| candidate.persistent_id.is_some_and(|id| id.is_nil()))
            || candidates.iter().any(|candidate| candidate.persistent_id.is_some())
                && invalidator.is_none()
        {
            return Err(EhentaiError::credential(EhentaiOperation::CreateTopic));
        }
        Ok(Self { candidates: Arc::new(StdMutex::new(candidates)), invalidator })
    }

    /// 从一至两个本地密钥明文建立凭据候选集。
    pub fn from_secrets(first: &[u8], second: Option<&[u8]>) -> Result<Self, EhentaiError> {
        let mut cookies = vec![EhentaiCookie::from_bytes(first)?];
        if let Some(second) = second {
            cookies.push(EhentaiCookie::from_bytes(second)?);
        }
        Self::from_cookies(cookies)
    }

    /// 返回候选集是否为空。
    pub fn is_empty(&self) -> bool {
        self.candidates.lock().map(|candidates| candidates.is_empty()).unwrap_or(true)
    }

    pub(super) fn begin(&self) -> Result<EhentaiCredentialSession, EhentaiError> {
        let candidate = self
            .candidates
            .lock()
            .map_err(|_| EhentaiError::credential(EhentaiOperation::FetchTopic))?
            .first()
            .cloned()
            .ok_or_else(|| EhentaiError::credential(EhentaiOperation::FetchTopic))?;
        Ok(EhentaiCredentialSession {
            set: self.clone(),
            current: candidate.clone(),
            tried: HashSet::from([candidate.key]),
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
        let current = self.current.clone();
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
