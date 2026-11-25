//! 派发 executor 与补寄消费端的持久来源：[`DispatchWorkStore`] 的存储实现。
//!
//! 补寄新轮的正文要按冻结模板渲染，D0 又由备好事务的数据库时刻决定，所以渲染只能在同一事务里做：
//! 与购买首轮相同，内容来源（[`FirstDispatchContentSource`]）由装配根注入。其余三项只委托存储。

use std::sync::Arc;

use auction_application::consignment::{
    ConsignmentFuture, DispatchSendRequest, DispatchWorkClaim, DispatchWorkStore, QueuedRedispatch,
    RedispatchPrepared, ScanCursor, ScanPage,
};
use auction_time::Timestamp;
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::purchase::FirstDispatchContentSource;

/// 派发与补寄工作来源；持有存储与冻结模板内容来源。
#[derive(Clone)]
pub struct DispatchWorkRepository {
    store: DatabaseStore,
    content: Arc<dyn FirstDispatchContentSource>,
}

impl DispatchWorkRepository {
    /// 由装配根以存储与首轮同一内容来源构造。
    pub fn new(store: DatabaseStore, content: Arc<dyn FirstDispatchContentSource>) -> Self {
        Self { store, content }
    }
}

impl DispatchWorkStore for DispatchWorkRepository {
    fn claim_dispatch_work(
        &self,
        now: Timestamp,
        lease_until: Timestamp,
    ) -> ConsignmentFuture<'_, Option<DispatchWorkClaim>> {
        Box::pin(async move {
            self.store
                .claim_dispatch_obligation(now, lease_until)
                .await
                .map_err(WriteError::into_application)
        })
    }

    fn dispatch_send_request(
        &self,
        delivery_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<DispatchSendRequest>> {
        Box::pin(async move {
            self.store
                .load_dispatch_send_request(delivery_id)
                .await
                .map_err(WriteError::into_application)
        })
    }

    fn queued_redispatches(
        &self,
        after: Option<ScanCursor>,
        limit: u32,
    ) -> ConsignmentFuture<'_, ScanPage<QueuedRedispatch>> {
        Box::pin(async move {
            self.store
                .load_queued_redispatches(after, limit)
                .await
                .map_err(WriteError::into_application)
        })
    }

    fn prepare_redispatch(
        &self,
        dispatch_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<RedispatchPrepared>> {
        Box::pin(async move {
            self.store
                .prepare_redispatch_delivery(dispatch_id, &self.content)
                .await
                .map_err(WriteError::into_application)
        })
    }
}
