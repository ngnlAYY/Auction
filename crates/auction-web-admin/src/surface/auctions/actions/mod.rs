//! Admin 拍卖生命周期操作的稳定出口。页面持有草稿，表格行只负责打开对应编辑。

use dioxus::prelude::*;

use super::editor::DraftRead;
use crate::api::{AdminAuction, AdminAuctionLifecycleAction};

mod dialog;
mod model;
mod row;
mod schedule;
#[cfg(test)]
mod tests;

use dialog::{AdminAuctionActionDialog, LifecycleDraft, LifecycleTarget};
use schedule::{AdminAuctionScheduleDialog, ScheduleDraft};

pub(super) use model::{AdminAuctionNotice, auction_notice_label};
pub(super) use row::AdminAuctionRow;

/// 页级草稿宿主。在读取失败、换页或对应行卸载时保留草稿，当前页只打开一个编辑框。
#[derive(Clone, Copy, PartialEq)]
pub(super) struct AuctionDrafts {
    lifecycle: Signal<Option<LifecycleDraft>>,
    schedule: Signal<Option<ScheduleDraft>>,
}

pub(super) fn use_auction_drafts() -> AuctionDrafts {
    let lifecycle = use_signal(|| None);
    let schedule = use_signal(|| None);
    use_context_provider(|| AuctionDrafts { lifecycle, schedule })
}

impl AuctionDrafts {
    pub(super) fn auction_id(self) -> Option<String> {
        self.schedule
            .read()
            .as_ref()
            .map(|draft| draft.auction_id().to_owned())
            .or_else(|| self.lifecycle.read().as_ref().map(|draft| draft.auction_id().to_owned()))
    }

    fn is_open(self) -> bool {
        self.schedule.peek().is_some() || self.lifecycle.peek().is_some()
    }

    fn open_schedule(mut self, auction: &AdminAuction) -> bool {
        if self.is_open() {
            return false;
        }
        let Some(draft) = ScheduleDraft::from_auction(auction) else {
            return false;
        };
        self.schedule.set(Some(draft));
        true
    }

    fn open_lifecycle(
        mut self,
        target: LifecycleTarget,
        action: AdminAuctionLifecycleAction,
    ) -> bool {
        if self.is_open() {
            return false;
        }
        self.lifecycle.set(Some(LifecycleDraft::new(target, action)));
        true
    }
}

#[component]
pub(super) fn AdminAuctionDraftDialogs(
    drafts: AuctionDrafts,
    latest_version: Option<i64>,
    read: DraftRead,
    notice: Signal<Option<AdminAuctionNotice>>,
    reload_generation: Signal<u64>,
) -> Element {
    rsx! {
        if drafts.schedule.read().is_some() {
            AdminAuctionScheduleDialog {
                draft: drafts.schedule,
                latest_version,
                read: read.clone(),
                notice,
                reload_generation,
            }
        }
        if drafts.lifecycle.read().is_some() {
            AdminAuctionActionDialog {
                latest_version,
                read,
                notice,
                pending: drafts.lifecycle,
                reload_generation,
            }
        }
    }
}
