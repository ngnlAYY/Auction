//! 论坛同步运行时的依赖装配与预算校验。

use std::sync::{Arc, OnceLock};

use auction_application::forum::{
    ForumCredentialAvailability, ForumPostWriter, ForumPublisher, ForumRenderer, ForumSyncStore,
    ForumTopicReader,
};
use auction_application::public::PublicEquipmentLinkResolver;

use crate::worker::runtime::WorkerError;

use super::links::valid_public_base_url;

pub(super) const MAX_BATCH_SIZE: u16 = 100;

/// 论坛主题同步运行时依赖。
pub struct ForumSyncPorts {
    pub store: Arc<dyn ForumSyncStore>,
    pub credential_availability: Arc<dyn ForumCredentialAvailability>,
    pub publisher: Arc<dyn ForumPublisher>,
    pub reader: Arc<dyn ForumTopicReader>,
    pub post_writer: Arc<dyn ForumPostWriter>,
    pub renderer: Arc<dyn ForumRenderer>,
    pub equipment_links: Arc<dyn PublicEquipmentLinkResolver>,
}

pub struct ForumSyncRuntime {
    pub(crate) store: Arc<dyn ForumSyncStore>,
    pub(crate) credential_availability: Arc<dyn ForumCredentialAvailability>,
    pub(crate) publisher: Arc<dyn ForumPublisher>,
    pub(crate) reader: Arc<dyn ForumTopicReader>,
    pub(crate) post_writer: Arc<dyn ForumPostWriter>,
    pub(crate) renderer: Arc<dyn ForumRenderer>,
    pub(crate) equipment_links: Arc<dyn PublicEquipmentLinkResolver>,
    pub(crate) batch_size: u16,
    pub(crate) item_timeout: std::time::Duration,
    pub(crate) public_base_url: Option<String>,
    pub(crate) verify_post_body: bool,
    pub(crate) startup_recovery_before: OnceLock<chrono::DateTime<chrono::Utc>>,
}

impl ForumSyncRuntime {
    /// 建立论坛同步运行时，并限制批次规模、单项处理时限和公开链接 origin。
    pub fn new(
        ports: ForumSyncPorts,
        batch_size: u16,
        item_timeout: std::time::Duration,
        public_base_url: Option<String>,
    ) -> Result<Self, WorkerError> {
        if batch_size == 0
            || batch_size > MAX_BATCH_SIZE
            || item_timeout.is_zero()
            || public_base_url.as_deref().is_some_and(|value| !valid_public_base_url(value))
        {
            return Err(WorkerError::InvalidForumSyncConfiguration);
        }
        let ForumSyncPorts {
            store,
            credential_availability,
            publisher,
            reader,
            post_writer,
            renderer,
            equipment_links,
        } = ports;
        Ok(Self {
            store,
            credential_availability,
            publisher,
            reader,
            post_writer,
            renderer,
            equipment_links,
            batch_size,
            item_timeout,
            public_base_url,
            verify_post_body: false,
            startup_recovery_before: OnceLock::new(),
        })
    }

    /// 配置固定帖原始正文校验；默认关闭，主题身份、回执及未知写入禁止重发的门禁始终保留。
    pub fn with_post_body_verification(mut self, enabled: bool) -> Self {
        self.verify_post_body = enabled;
        self
    }
}
