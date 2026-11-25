//! 自动重列用例：规范摘要、系统命令 UUIDv5 与端口命令组装。

use std::sync::Arc;

use auction_domain::CommandId;
use uuid::Uuid;

use super::resolve::{Stream, system_id};
use crate::ports::consignment::{
    AutoRelist, AutoRelistReceipt, AutoRelistStore, ConsignmentError, PendingAutoRelist,
    RecoveryOperation, ScanCursor, ScanPage,
};
use crate::ports::core::RequestHash;

/// `auto_relist`：业务键为未付收回处置 ID；摘要含处置与目标挂单。额度上限是运行参数，不进摘要。
fn relist_command(disposition_id: Uuid, listing_id: Uuid) -> (CommandId, RequestHash) {
    let mut stream = Stream::new(RecoveryOperation::AutoRelist, listing_id);
    stream.uuid(disposition_id);
    (system_id(RecoveryOperation::AutoRelist, disposition_id.as_bytes()), stream.request_hash())
}

/// 自动重列写用例；只依赖窄端口。
pub struct AutoRelistService {
    store: Arc<dyn AutoRelistStore>,
}

impl AutoRelistService {
    /// 注入自动重列写端口。
    pub fn new(store: Arc<dyn AutoRelistStore>) -> Self {
        Self { store }
    }

    /// 对一个未付收回处置作自动重列决定。
    pub async fn relist(
        &self,
        pending: PendingAutoRelist,
        max_open_per_user: i64,
    ) -> Result<AutoRelistReceipt, ConsignmentError> {
        let (command_id, request_hash) = relist_command(pending.disposition_id, pending.listing_id);
        self.store
            .auto_relist(AutoRelist {
                command_id,
                request_hash,
                disposition_id: pending.disposition_id,
                listing_id: pending.listing_id,
                max_open_per_user,
            })
            .await
    }

    /// 游标之后待自动重列的处置（一页）。
    pub async fn pending(
        &self,
        after: Option<ScanCursor>,
        limit: u32,
    ) -> Result<ScanPage<PendingAutoRelist>, ConsignmentError> {
        self.store.pending_auto_relists(after, limit).await
    }
}
