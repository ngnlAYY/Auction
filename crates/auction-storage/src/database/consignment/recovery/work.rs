//! 恢复处置三个端口在 `DatabaseStore` 上的实现入口：只做分派，规则与 SQL 在各自的子模块。

use auction_application::consignment::{
    AutoRelist, AutoRelistReceipt, AutoRelistStore, ConsignmentFuture, PaymentReminderStore,
    PendingAutoRelist, PendingResolution, RemindContract, ReminderReceipt, ResolveReceipt,
    ResolveReturn, ReturnDispositionStore, ScanCursor, ScanPage,
};
use uuid::Uuid;

use super::{relist, reminder, resolve};
use crate::DatabaseStore;

impl ReturnDispositionStore for DatabaseStore {
    fn resolve_return(&self, command: ResolveReturn) -> ConsignmentFuture<'_, ResolveReceipt> {
        Box::pin(resolve::resolve_return(self, command))
    }

    fn pending_resolutions(
        &self,
        after: Option<ScanCursor>,
        limit: u32,
    ) -> ConsignmentFuture<'_, ScanPage<PendingResolution>> {
        Box::pin(resolve::pending_resolutions(self, after, limit))
    }
}

impl AutoRelistStore for DatabaseStore {
    fn auto_relist(&self, command: AutoRelist) -> ConsignmentFuture<'_, AutoRelistReceipt> {
        Box::pin(relist::auto_relist(self, command))
    }

    fn pending_auto_relists(
        &self,
        after: Option<ScanCursor>,
        limit: u32,
    ) -> ConsignmentFuture<'_, ScanPage<PendingAutoRelist>> {
        Box::pin(relist::pending_auto_relists(self, after, limit))
    }
}

impl PaymentReminderStore for DatabaseStore {
    fn remind(&self, command: RemindContract) -> ConsignmentFuture<'_, ReminderReceipt> {
        Box::pin(reminder::remind(self, command))
    }

    fn due_reminders(
        &self,
        after: Option<ScanCursor>,
        limit: u32,
    ) -> ConsignmentFuture<'_, ScanPage<Uuid>> {
        Box::pin(reminder::due_reminders(self, after, limit))
    }
}
