use std::{
    collections::{HashSet, VecDeque},
    sync::{Arc, Mutex},
};

use auction_application::AdminCredentialRuntimeWriter;
use chrono::Utc;
use uuid::Uuid;

use super::{
    MoogleMailClient, MoogleMailCookie, MoogleMailError, MoogleMailErrorCategory,
    MoogleMailMessage, MoogleMailOperation, MoogleMailParser, MoogleMailWorld,
    protocol::valid_opaque_id,
};

const MAX_HARD_PAGES: usize = 100;
const MAX_FETCH_LIMIT: usize = 1000;

/// MoogleMail 收件箱 gateway 的有界扫描配置。
#[derive(Debug, Clone)]
pub struct MoogleMailGatewayConfig {
    /// 单次逻辑拉取最多读取的页数。
    pub maximum_pages: usize,
    /// 发件人 allowlist；默认只解析 allowlist 内邮件的详情。
    pub sender_allowlist: Vec<String>,
    /// 明确关闭 allowlist 后才允许读取所有发件人详情。
    pub sender_allowlist_disabled: bool,
}

impl Default for MoogleMailGatewayConfig {
    fn default() -> Self {
        Self {
            maximum_pages: MAX_HARD_PAGES,
            sender_allowlist: Vec::new(),
            sender_allowlist_disabled: false,
        }
    }
}

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

/// 已完成摘要/详情交叉校验的一次有界邮件拉取。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailPage {
    /// 按最旧到最新顺序返回，便于应用层顺序入库。
    pub messages: Vec<MoogleMailMessage>,
    /// 本批最新已观测摘要的 external ID；空批由调用方保留旧 cursor。
    pub next_cursor: Option<String>,
}

/// MoogleMail 收件箱读取 gateway。
pub struct MoogleMailGateway {
    client: Arc<MoogleMailClient>,
    credentials: MoogleMailCredentialSet,
    config: MoogleMailGatewayConfig,
    parser: MoogleMailParser,
}

impl MoogleMailGateway {
    /// 构造一个固定 world 由调用方传入的通用 gateway。
    pub fn new(
        client: Arc<MoogleMailClient>,
        credentials: MoogleMailCredentialSet,
        config: MoogleMailGatewayConfig,
    ) -> Result<Self, MoogleMailError> {
        if config.maximum_pages == 0 || config.maximum_pages > MAX_HARD_PAGES {
            return Err(configuration(MoogleMailWorld::Main));
        }
        for (index, sender) in config.sender_allowlist.iter().enumerate() {
            if sender.is_empty()
                || sender != sender.trim()
                || sender.len() > 256
                || sender.chars().any(char::is_control)
                || config.sender_allowlist[..index]
                    .iter()
                    .any(|previous| previous.eq_ignore_ascii_case(sender))
            {
                return Err(configuration(MoogleMailWorld::Main));
            }
        }
        Ok(Self { client, credentials, config, parser: MoogleMailParser })
    }

    /// 读取一个 world 的新邮件，cursor 只在完整批次成功后由调用方提交。
    pub async fn fetch(
        &self,
        world: MoogleMailWorld,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<MoogleMailPage, MoogleMailError> {
        if limit == 0
            || limit > MAX_FETCH_LIMIT
            || cursor.is_some_and(|value| !valid_opaque_id(value))
        {
            return Err(configuration(world));
        }
        let mut session = self.credentials.begin(world)?;
        for _ in 0..64 {
            match self.fetch_with_cookie(world, cursor, limit, session.cookie()).await {
                Ok(page) => return Ok(page),
                Err(error) if error.category == MoogleMailErrorCategory::SessionExpired => {
                    session.invalidate_current().await?;
                    if session.advance().is_err() {
                        return Err(error);
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(operation_error(
            super::MoogleMailOperation::Inbox,
            world,
            MoogleMailErrorCategory::CredentialUnavailable,
        ))
    }

    /// 按 exact remote ID 扫描 world-specific Sent 列表并读取详情证明。
    pub async fn read_sent(
        &self,
        world: MoogleMailWorld,
        external_mail_id: &str,
        observed_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<super::MoogleMailSentSnapshot, MoogleMailError> {
        if !valid_opaque_id(external_mail_id) || observed_at.timestamp_nanos_opt().is_none() {
            return Err(operation_error(
                MoogleMailOperation::Sent,
                world,
                MoogleMailErrorCategory::Configuration,
            ));
        }
        let mut session = self.credentials.begin(world)?;
        for _ in 0..64 {
            match self
                .read_sent_with_cookie(world, external_mail_id, observed_at, session.cookie())
                .await
            {
                Ok(snapshot) => return Ok(snapshot),
                Err(error) if error.category == MoogleMailErrorCategory::SessionExpired => {
                    session.invalidate_current().await?;
                    if session.advance().is_err() {
                        return Err(error);
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(operation_error(
            MoogleMailOperation::Sent,
            world,
            MoogleMailErrorCategory::CredentialUnavailable,
        ))
    }

    /// 读取并校验一个 world 的 Item Shop Credits 余额。
    pub async fn read_balance(
        &self,
        world: MoogleMailWorld,
        observed_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<super::MoogleMailBalanceSnapshot, MoogleMailError> {
        if observed_at.timestamp_nanos_opt().is_none() {
            return Err(operation_error(
                MoogleMailOperation::Balance,
                world,
                MoogleMailErrorCategory::Configuration,
            ));
        }
        let mut session = self.credentials.begin(world)?;
        for _ in 0..64 {
            let result = async {
                let target = self.client.balance(world)?;
                let document = self.client.get(&target, session.cookie()).await?;
                self.parser.parse_balance(&document, world, observed_at)
            }
            .await;
            match result {
                Ok(snapshot) => return Ok(snapshot),
                Err(error) if error.category == MoogleMailErrorCategory::SessionExpired => {
                    session.invalidate_current().await?;
                    if session.advance().is_err() {
                        return Err(error);
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(operation_error(
            MoogleMailOperation::Balance,
            world,
            MoogleMailErrorCategory::CredentialUnavailable,
        ))
    }

    async fn read_sent_with_cookie(
        &self,
        world: MoogleMailWorld,
        external_mail_id: &str,
        observed_at: chrono::DateTime<chrono::Utc>,
        cookie: &MoogleMailCookie,
    ) -> Result<super::MoogleMailSentSnapshot, MoogleMailError> {
        let mut seen = HashSet::new();
        for page_index in 0..self.config.maximum_pages {
            let target = self.client.sent_page(world, page_index)?;
            let document = self.client.get(&target, cookie).await?;
            let sent_page = self.parser.scan_sent_page(&document, world, page_index, &mut seen)?;
            if let Some(summary) = sent_page
                .summaries
                .iter()
                .find(|summary| summary.external_mail_id == external_mail_id)
            {
                let detail_target = self.client.sent_detail(world, external_mail_id)?;
                let detail_document = self.client.get(&detail_target, cookie).await?;
                let snapshot =
                    self.parser.parse_sent_detail(&detail_document, summary, observed_at)?;
                if snapshot.external_mail_id != external_mail_id
                    || snapshot.world != world
                    || !snapshot.recipient.eq_ignore_ascii_case(&summary.recipient)
                    || snapshot.subject != summary.subject
                    || snapshot.sent_at != summary.sent_at
                {
                    return Err(operation_error(
                        MoogleMailOperation::SentDetail,
                        world,
                        MoogleMailErrorCategory::IdentityMismatch,
                    ));
                }
                return Ok(snapshot);
            }
            if !sent_page.has_next_page {
                return Err(operation_error(
                    MoogleMailOperation::Sent,
                    world,
                    MoogleMailErrorCategory::NotObserved,
                ));
            }
        }
        Err(operation_error(
            MoogleMailOperation::Sent,
            world,
            MoogleMailErrorCategory::ProtocolDrift,
        ))
    }

    async fn fetch_with_cookie(
        &self,
        world: MoogleMailWorld,
        cursor: Option<&str>,
        limit: usize,
        cookie: &MoogleMailCookie,
    ) -> Result<MoogleMailPage, MoogleMailError> {
        let mut seen = HashSet::new();
        let mut window = VecDeque::with_capacity(limit);
        let mut found_cursor = false;
        let mut reached_end = false;
        for page_index in 0..self.config.maximum_pages {
            let target = self.client.inbox_page(world, page_index)?;
            let document = self.client.get(&target, cookie).await?;
            let inbox_page =
                self.parser.scan_inbox_page(&document, world, page_index, &mut seen)?;
            for summary in inbox_page.summaries {
                if found_cursor {
                    continue;
                }
                if cursor.is_some_and(|value| value == summary.external_mail_id) {
                    found_cursor = true;
                    continue;
                }
                // allowlist 必须在占用详情配额前生效，不能让大量无关发件人挤掉真正需要解析的邮件。
                if self.allowed_sender(&summary.sender) && window.len() < limit {
                    window.push_back(summary);
                }
            }
            if found_cursor {
                break;
            }
            if !inbox_page.has_next_page {
                reached_end = true;
                break;
            }
        }
        if !found_cursor && !reached_end {
            return Err(protocol(world));
        }
        let newest = window.front().map(|summary| summary.external_mail_id.clone());
        let mut messages = Vec::with_capacity(window.len());
        for summary in window.into_iter().rev() {
            let target = self.client.detail(world, &summary.external_mail_id)?;
            let document = self.client.get(&target, cookie).await?;
            let message = self.parser.parse_detail(&document, &summary)?;
            if message.external_mail_id != summary.external_mail_id
                || message.world != summary.world
                || message.received_at != summary.received_at
                || !message.sender.eq_ignore_ascii_case(&summary.sender)
            {
                return Err(MoogleMailError::new(
                    super::MoogleMailOperation::Detail,
                    world,
                    MoogleMailErrorCategory::IdentityMismatch,
                    false,
                ));
            }
            messages.push(message);
        }
        // 没有任何 allowlist 命中的详情时不能推进 cursor，否则后续补充 allowlist 会永久错过
        // 已经扫描过的邮件；有成功解析的邮件时才允许跳过同一窗口内明确忽略的发件人。
        let next_cursor = if messages.is_empty() { cursor.map(str::to_owned) } else { newest };
        Ok(MoogleMailPage { messages, next_cursor })
    }

    fn allowed_sender(&self, sender: &str) -> bool {
        self.config.sender_allowlist_disabled
            || self
                .config
                .sender_allowlist
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(sender))
    }
}

fn configuration(world: MoogleMailWorld) -> MoogleMailError {
    operation_error(
        super::MoogleMailOperation::Inbox,
        world,
        MoogleMailErrorCategory::Configuration,
    )
}

fn protocol(world: MoogleMailWorld) -> MoogleMailError {
    operation_error(
        super::MoogleMailOperation::Inbox,
        world,
        MoogleMailErrorCategory::ProtocolDrift,
    )
}

fn operation_error(
    operation: MoogleMailOperation,
    world: MoogleMailWorld,
    category: MoogleMailErrorCategory,
) -> MoogleMailError {
    MoogleMailError::new(operation, world, category, false)
}
