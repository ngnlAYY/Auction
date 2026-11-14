//! 论坛回帖导入 Worker 的模块装配；runtime 配置与 `run_cycle` 入口在此，游标推进和
//! warning-only 边界不变。

use std::sync::Arc;

use auction_application::auction::CompetitionCommandPort;
use auction_application::forum::{
    ForumCredentialAvailability, ForumPostWriter, ForumRenderer, ForumReplyStore, ForumTopicReader,
};
use auction_application::public::PublicEquipmentLinkResolver;

use crate::worker::runtime::WorkerError;

mod command;
mod completion;
mod notification;
mod rejection;

mod cycle;
mod topic;
pub(crate) use cycle::run_cycle;

use super::reply_delay::ReplyDelayEstimator;

const MAX_BATCH_SIZE: u16 = 100;

/// 单条论坛命令已经证明的帖子、身份与 lot 上下文。
struct ForumCommandContext<'a> {
    topic: &'a auction_application::forum::ForumReplyTopic,
    bidder: &'a auction_application::forum::ForumBidderIdentity,
    post_id: &'a str,
    post_number: usize,
    lot_code: &'a str,
    command: &'a auction_application::admin::AdminForumCommand,
    lot_id: auction_domain::LotId,
}

/// 论坛回帖导入运行时依赖。
pub struct ForumReplyPorts {
    pub store: Arc<dyn ForumReplyStore>,
    pub credential_availability: Arc<dyn ForumCredentialAvailability>,
    pub reader: Arc<dyn ForumTopicReader>,
    pub writer: Arc<dyn ForumPostWriter>,
    pub renderer: Arc<dyn ForumRenderer>,
    pub equipment_links: Arc<dyn PublicEquipmentLinkResolver>,
    pub commands: Arc<dyn CompetitionCommandPort>,
}

pub struct ForumReplyRuntime {
    pub(crate) store: Arc<dyn ForumReplyStore>,
    pub(crate) credential_availability: Arc<dyn ForumCredentialAvailability>,
    pub(crate) reader: Arc<dyn ForumTopicReader>,
    pub(crate) writer: Arc<dyn ForumPostWriter>,
    pub(crate) renderer: Arc<dyn ForumRenderer>,
    pub(crate) equipment_links: Arc<dyn PublicEquipmentLinkResolver>,
    pub(crate) commands: Arc<dyn CompetitionCommandPort>,
    pub(crate) batch_size: u16,
    pub(crate) item_timeout: std::time::Duration,
    pub(crate) public_base_url: Option<String>,
    pub(crate) verify_post_body: bool,
    /// 说明信预算窗口与上限，默认 24 小时 10 封；论坛回执与 MoogleMail 说明信共用计数。
    pub(crate) external_metrics:
        Arc<dyn auction_application::external_metrics::ExternalMetricsObserver>,
    reply_delay: ReplyDelayEstimator,
}

impl ForumReplyRuntime {
    /// 建立论坛回帖导入运行时，并限制单轮主题读取数量。
    pub fn new(
        ports: ForumReplyPorts,
        batch_size: u16,
        item_timeout: std::time::Duration,
        public_base_url: Option<String>,
    ) -> Result<Self, WorkerError> {
        if batch_size == 0
            || batch_size > MAX_BATCH_SIZE
            || item_timeout.is_zero()
            || public_base_url
                .as_deref()
                .is_some_and(|value| !super::sync::valid_public_base_url(value))
        {
            return Err(WorkerError::InvalidForumReplyConfiguration);
        }
        let ForumReplyPorts {
            store,
            credential_availability,
            reader,
            writer,
            renderer,
            equipment_links,
            commands,
        } = ports;
        Ok(Self {
            store,
            credential_availability,
            reader,
            writer,
            renderer,
            equipment_links,
            commands,
            batch_size,
            item_timeout,
            public_base_url,
            verify_post_body: false,
            external_metrics: auction_application::external_metrics::noop_external_metrics_observer(
            ),
            reply_delay: ReplyDelayEstimator::new(
                auction_application::external_metrics::noop_external_metrics_observer(),
            ),
        })
    }

    /// 配置正常固定帖刷新的额外正文校验；未知写入始终只读核对。
    /// 覆盖说明信预算；阈值来自 Worker 配置，仍只影响说明信，不改变命令与资产退回。
    pub fn with_post_body_verification(mut self, enabled: bool) -> Self {
        self.verify_post_body = enabled;
        self
    }

    /// 注入同步外部客户端指标；延迟估计只读内存，不改变扫描或命令控制流。
    pub fn with_external_metrics(
        mut self,
        observer: Arc<dyn auction_application::external_metrics::ExternalMetricsObserver>,
    ) -> Self {
        self.reply_delay = ReplyDelayEstimator::new(Arc::clone(&observer));
        self.external_metrics = observer;
        self
    }

    /// 共享同一论坛读取适配器与限速，供恢复核对只读扫描使用。
    pub fn topic_reader(&self) -> Arc<dyn ForumTopicReader> {
        Arc::clone(&self.reader)
    }

    /// 记录论坛单项预算耗尽；只读同步计数，不改变截止点。
    pub(crate) fn observe_budget(
        &self,
        world: auction_domain::SettlementWorld,
        phase: auction_application::external_metrics::ForumBudgetPhase,
    ) {
        let world = match world {
            auction_domain::SettlementWorld::Main => {
                auction_application::external_metrics::ForumWorld::Main
            }
            auction_domain::SettlementWorld::Isekai => {
                auction_application::external_metrics::ForumWorld::Isekai
            }
        };
        self.external_metrics.observe(
            auction_application::external_metrics::ExternalMetricsEvent::ForumBudgetExhausted {
                world,
                phase,
            },
        );
    }
}
