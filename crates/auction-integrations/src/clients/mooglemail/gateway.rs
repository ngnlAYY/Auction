use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
};

use super::{
    MoogleMailClient, MoogleMailCookie, MoogleMailError, MoogleMailErrorCategory,
    MoogleMailMessage, MoogleMailParser, MoogleMailWorld, protocol::valid_opaque_id,
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

/// 两个本地 `.keyenv` 身份的受控 Cookie 集合。
///
/// `bot` 对应 `hv-ck-key1`，`test` 对应 `hv-ck-key2`。集合只保存脱敏 Cookie 值对象，不
/// 实现 `Serialize`，也不提供把值写入日志或 DTO 的接口。
#[derive(Clone)]
pub struct MoogleMailCredentialSet {
    bot: MoogleMailCookie,
    test: MoogleMailCookie,
}

impl MoogleMailCredentialSet {
    /// 由配置层显式 getter 建立 Bot/测试身份集合。
    pub fn new(bot: &str, test: &str) -> Result<Self, MoogleMailError> {
        Ok(Self { bot: MoogleMailCookie::new(bot)?, test: MoogleMailCookie::new(test)? })
    }

    fn ordered(&self) -> [&MoogleMailCookie; 2] {
        [&self.bot, &self.test]
    }
}

impl std::fmt::Debug for MoogleMailCredentialSet {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MoogleMailCredentialSet([REDACTED; 2])")
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
        let mut last_session_error = None;
        for cookie in self.credentials.ordered() {
            match self.fetch_with_cookie(world, cursor, limit, cookie).await {
                Ok(page) => return Ok(page),
                Err(error) if error.category == MoogleMailErrorCategory::SessionExpired => {
                    last_session_error = Some(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_session_error.unwrap_or_else(|| {
            MoogleMailError::new(
                super::MoogleMailOperation::Inbox,
                world,
                MoogleMailErrorCategory::CredentialUnavailable,
                false,
            )
        }))
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
    MoogleMailError::new(
        super::MoogleMailOperation::Inbox,
        world,
        MoogleMailErrorCategory::Configuration,
        false,
    )
}

fn protocol(world: MoogleMailWorld) -> MoogleMailError {
    MoogleMailError::new(
        super::MoogleMailOperation::Inbox,
        world,
        MoogleMailErrorCategory::ProtocolDrift,
        false,
    )
}
