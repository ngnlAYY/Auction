//! gateway 构造与观测注入：只做配置校验与状态初始化，不发起远端读取。

use std::{
    collections::HashMap,
    sync::{Arc, Mutex as StdMutex},
};

use auction_application::mail::{MailPollProgressObserver, MailScanObserver};

use super::MoogleMailGateway;
use crate::clients::mooglemail::client::MoogleMailClient;
use crate::clients::mooglemail::error::MoogleMailError;
use crate::clients::mooglemail::gateway::credentials::MoogleMailCredentialSet;
use crate::clients::mooglemail::gateway::model::MoogleMailGatewayConfig;
use crate::clients::mooglemail::gateway::{MAX_HARD_PAGES, configuration};
use crate::clients::mooglemail::parser::MoogleMailParser;
use crate::clients::mooglemail::protocol::MoogleMailWorld;

impl MoogleMailGateway {
    /// 配置的列表页数硬上限；恢复核对只能在同一上限内请求更小预算。
    pub fn maximum_pages(&self) -> usize {
        self.config.maximum_pages
    }

    /// 构造一个固定 world 由调用方传入的通用 gateway。
    pub fn new(
        client: Arc<MoogleMailClient>,
        credentials: MoogleMailCredentialSet,
        config: MoogleMailGatewayConfig,
    ) -> Result<Self, MoogleMailError> {
        if config.maximum_pages == 0
            || config.maximum_pages > MAX_HARD_PAGES
            || config.fetch_budget.is_zero()
        {
            return Err(configuration(MoogleMailWorld::Main));
        }
        for (index, sender) in config.sender_allowlist.iter().enumerate() {
            if !auction_domain::valid_text(sender, 256)
                || config.sender_allowlist[..index]
                    .iter()
                    .any(|previous| previous.eq_ignore_ascii_case(sender))
            {
                return Err(configuration(MoogleMailWorld::Main));
            }
        }
        let client = Arc::new(client.as_ref().clone().with_ownership(credentials.ownership()));
        Ok(Self {
            client,
            credentials,
            config,
            parser: MoogleMailParser,
            profile_reader: None,
            progress_observer: None,
            scan_checkpoints: StdMutex::new(HashMap::new()),
            scan_observer: auction_application::mail::noop_mail_scan_observer(),
            sent_page_hints: super::sent_hints::SentPageHints::default(),
        })
    }

    /// 注入增量扫描的闭集观测；观察失败不改变读取或证明结果。
    pub fn with_scan_observer(mut self, observer: Arc<dyn MailScanObserver>) -> Self {
        self.scan_observer = observer;
        self
    }

    /// 注入进程内观测；仅持有 claim 的轮询路径报告真实读取目标，不参与协议或授权决定。
    pub fn with_progress_observer(mut self, observer: Arc<dyn MailPollProgressObserver>) -> Self {
        self.progress_observer = Some(observer);
        self
    }
}
