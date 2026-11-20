//! 结算页的四项 GET、请求身份与各自的重试入口。

use dioxus::prelude::*;

use super::read_retry::{SettlementRead, SettlementReadRetry, focus_after_successful_read};
use super::selection::{CatalogSelection, report_for_selection, resolve_catalog_selection};
#[cfg(not(test))]
use crate::api::fetch_admin_settlement_catalog;
use crate::api::{
    AdminRecoveryReport, AdminSettlementCatalogPage, AdminSettlementReport, fetch_admin_recovery,
    fetch_admin_settlement,
};

#[cfg(test)]
use super::tests::fetch_admin_settlement_catalog;
use crate::surface::admin_api_error_message;

/// 分页目录快照，以请求身份绑定当前筛选。
pub(super) struct CatalogLoad {
    pub(super) world_id: String,
    pub(super) page: i64,
    pub(super) page_size: i64,
    pub(super) result: Result<AdminSettlementCatalogPage, String>,
}

/// 按期号补查的结果，以请求世界与期号核对身份。
pub(super) struct CatalogLookupLoad {
    pub(super) world_id: String,
    pub(super) sequence: Option<i64>,
    pub(super) result: Result<Option<AdminSettlementCatalogPage>, String>,
}

/// 场次报告快照，切换 UUID 后不再展示。
pub(super) struct DetailLoad<T> {
    pub(super) auction_id: String,
    pub(super) result: Result<Option<T>, String>,
}

/// 四项读取所用的当前筛选与场次选择。
#[derive(Clone, Copy)]
pub(super) struct SettlementReadQuery {
    pub(super) world: Signal<String>,
    pub(super) page: Signal<i64>,
    pub(super) page_size: Signal<i64>,
    pub(super) auction_id: Signal<String>,
    pub(super) sequence: Signal<Option<i64>>,
}

/// 独立的读取资源与恢复报告的版本世代。
#[derive(Clone, Copy)]
pub(super) struct SettlementReads {
    pub(super) catalog: Resource<CatalogLoad>,
    pub(super) lookup: Resource<CatalogLookupLoad>,
    pub(super) settlement: Resource<DetailLoad<AdminSettlementReport>>,
    pub(super) recovery: Resource<((u64, u64), DetailLoad<AdminRecoveryReport>)>,
    query: SettlementReadQuery,
    lookup_sequence: Memo<Option<i64>>,
    reload: Signal<u64>,
    recovery_reload: Signal<u64>,
}

/// 结算与恢复的确认入口仍须由目录与报告的同一条场次事实授权。
pub(super) fn use_settlement_reads(
    query: SettlementReadQuery,
    reload: Signal<u64>,
    recovery_reload: Signal<u64>,
) -> SettlementReads {
    let catalog = use_resource(move || {
        let generation = reload();
        let (world_id, page, page_size) = ((query.world)(), (query.page)(), (query.page_size)());
        async move {
            let result =
                fetch_admin_settlement_catalog(world_id.clone(), None, page, page_size).await;
            if result.is_ok()
                && generation == *reload.peek()
                && query.matches_catalog(&world_id, page, page_size)
            {
                let scope = catalog_scope(&world_id, page, page_size);
                focus_after_successful_read(SettlementRead::Catalog, &scope);
            }
            CatalogLoad { world_id, page, page_size, result }
        }
    });
    // 只有当前目录页无法解析期号时才补查，默认选择与页内选择不发额外请求。
    let lookup_sequence = use_memo(move || {
        let sequence = (query.sequence)()?;
        let loaded = catalog.read();
        let load = loaded.as_ref()?;
        let current = load.world_id == *query.world.read()
            && load.page == (query.page)()
            && load.page_size == (query.page_size)();
        let page = load.result.as_ref().ok().filter(|_| current)?;
        let selected = query.auction_id.read();
        matches!(
            resolve_catalog_selection(Some(page), None, &selected, Some(sequence)),
            CatalogSelection::Resolving
        )
        .then_some(sequence)
    });
    let lookup = use_resource(move || {
        let world_id = (query.world)();
        let sequence = lookup_sequence();
        async move {
            let result = match sequence {
                Some(sequence) => {
                    let loaded =
                        fetch_admin_settlement_catalog(world_id.clone(), Some(sequence), 1, 10)
                            .await;
                    let current = world_id == *query.world.peek()
                        && Some(sequence) == *query.sequence.peek()
                        && Some(sequence) == *lookup_sequence.peek();
                    if loaded.is_ok() && current {
                        let scope = lookup_scope(&world_id, Some(sequence));
                        focus_after_successful_read(SettlementRead::CatalogLookup, &scope);
                    }
                    loaded.map(Some)
                }
                None => Ok(None),
            };
            CatalogLookupLoad { world_id, sequence, result }
        }
    });
    let settlement = use_resource(move || {
        let generation = reload();
        let auction_id = (query.auction_id)();
        async move {
            let result = if auction_id.is_empty() {
                Ok(None)
            } else {
                fetch_admin_settlement(auction_id.clone()).await.map(Some)
            };
            // 身份不符的报告仍失败关闭，保留其重试按钮与焦点。
            let current = generation == *reload.peek() && auction_id == *query.auction_id.peek();
            if let Ok(Some(report)) = &result
                && current
                && query.with_selection(catalog, lookup, |selection| {
                    report_for_selection(selection, Some(report)).is_ok_and(|value| value.is_some())
                })
            {
                focus_after_successful_read(SettlementRead::Settlement, &auction_id);
            }
            DetailLoad { auction_id, result }
        }
    });
    let recovery = use_resource(move || {
        let generation = (reload(), recovery_reload());
        let auction_id = (query.auction_id)();
        async move {
            let result = if auction_id.is_empty() {
                Ok(None)
            } else {
                fetch_admin_recovery(auction_id.clone()).await.map(Some)
            };
            let current = generation == (*reload.peek(), *recovery_reload.peek())
                && auction_id == *query.auction_id.peek();
            if result.as_ref().is_ok_and(Option::is_some)
                && current
                && query.with_selection(catalog, lookup, |selection| {
                    matches!(selection, CatalogSelection::Matched(_))
                })
            {
                let scope = recovery_scope(&auction_id, generation);
                focus_after_successful_read(SettlementRead::Recovery, &scope);
            }
            (generation, DetailLoad { auction_id, result })
        }
    });
    SettlementReads {
        catalog,
        lookup,
        settlement,
        recovery,
        query,
        lookup_sequence,
        reload,
        recovery_reload,
    }
}

impl SettlementReadQuery {
    fn matches_catalog(self, world: &str, page: i64, page_size: i64) -> bool {
        world == self.world.peek().as_str()
            && page == *self.page.peek()
            && page_size == *self.page_size.peek()
    }

    fn with_selection<T>(
        self,
        catalog: Resource<CatalogLoad>,
        lookup: Resource<CatalogLookupLoad>,
        for_selection: impl FnOnce(CatalogSelection<'_>) -> T,
    ) -> T {
        let catalog = catalog.peek();
        let page = catalog
            .as_ref()
            .filter(|load| self.matches_catalog(&load.world_id, load.page, load.page_size))
            .and_then(|load| load.result.as_ref().ok());
        let sequence = *self.sequence.peek();
        let lookup = lookup.peek();
        let lookup = matching_lookup(lookup.as_ref(), &self.world.peek(), sequence)
            .and_then(|load| load.result.as_ref().ok())
            .and_then(Option::as_ref);
        for_selection(resolve_catalog_selection(page, lookup, &self.auction_id.peek(), sequence))
    }
}

impl SettlementReads {
    /// 按读取单独订阅在途状态，供原位的错误面板拦截重复激活。
    pub(super) fn pending(self, read: SettlementRead) -> bool {
        let state = match read {
            SettlementRead::Catalog => self.catalog.state(),
            SettlementRead::CatalogLookup => self.lookup.state(),
            SettlementRead::Settlement => self.settlement.state(),
            SettlementRead::Recovery => self.recovery.state(),
        };
        state.cloned() != UseResourceState::Ready
    }

    /// 错误面板持有原请求身份，同文案的其他读取不会接替它。
    pub(super) fn error(self, read: SettlementRead, code: &str) -> Element {
        let scope = self.scope(read);
        rsx! {
            SettlementReadRetry {
                read,
                scope: scope.clone(),
                message: admin_api_error_message(code),
                pending: self.pending(read),
                on_retry: move |()| self.retry(read, &scope),
            }
        }
    }

    fn scope(self, read: SettlementRead) -> String {
        match read {
            SettlementRead::Catalog => catalog_scope(
                &self.query.world.peek(),
                *self.query.page.peek(),
                *self.query.page_size.peek(),
            ),
            SettlementRead::CatalogLookup => {
                lookup_scope(&self.query.world.peek(), *self.query.sequence.peek())
            }
            SettlementRead::Settlement => self.query.auction_id.peek().clone(),
            SettlementRead::Recovery => recovery_scope(
                &self.query.auction_id.peek(),
                (*self.reload.peek(), *self.recovery_reload.peek()),
            ),
        }
    }

    fn retry(self, read: SettlementRead, scope: &str) {
        if scope != self.scope(read) {
            return;
        }
        match read {
            SettlementRead::Catalog => restart_failed_read(self.catalog, |load| {
                load.result.is_err()
                    && self.query.matches_catalog(&load.world_id, load.page, load.page_size)
            }),
            SettlementRead::CatalogLookup => {
                if self.lookup_sequence.peek().is_some() {
                    restart_failed_read(self.lookup, |load| {
                        load.result.is_err()
                            && load.world_id == *self.query.world.peek()
                            && load.sequence == *self.query.sequence.peek()
                    });
                }
            }
            SettlementRead::Settlement => restart_failed_read(self.settlement, |load| {
                load.auction_id == scope
                    && self.query.with_selection(self.catalog, self.lookup, |selection| {
                        matches!(selection, CatalogSelection::Matched(_))
                            && match &load.result {
                                Ok(report) => {
                                    report_for_selection(selection, report.as_ref()).is_err()
                                }
                                Err(_code) => true,
                            }
                    })
            }),
            SettlementRead::Recovery => restart_failed_read(self.recovery, |(generation, load)| {
                load.result.is_err()
                    && *generation == (*self.reload.peek(), *self.recovery_reload.peek())
                    && load.auction_id == *self.query.auction_id.peek()
                    && self.query.with_selection(self.catalog, self.lookup, |selection| {
                        matches!(selection, CatalogSelection::Matched(_))
                    })
            }),
        }
    }
}

fn restart_failed_read<T: 'static>(mut resource: Resource<T>, failed: impl FnOnce(&T) -> bool) {
    // restart 同步置为 Pending，拦住 DOM 更新前再次派发的点击。
    let retryable = *resource.state().peek() == UseResourceState::Ready
        && resource.peek().as_ref().is_some_and(failed);
    if retryable {
        resource.restart();
    }
}

fn catalog_scope(world: &str, page: i64, page_size: i64) -> String {
    format!("{world}:{page}:{page_size}")
}

fn lookup_scope(world: &str, sequence: Option<i64>) -> String {
    sequence.map_or_else(|| world.to_owned(), |sequence| format!("{world}:{sequence}"))
}

fn recovery_scope(auction_id: &str, (reload, recovery_reload): (u64, u64)) -> String {
    format!("{auction_id}:{reload}:{recovery_reload}")
}

/// 仅返回仍对应当前世界与期号的补查结果。
pub(super) fn matching_lookup<'a>(
    load: Option<&'a CatalogLookupLoad>,
    world: &str,
    sequence: Option<i64>,
) -> Option<&'a CatalogLookupLoad> {
    load.filter(|load| load.world_id == world && load.sequence == sequence)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use dioxus::prelude::{Callback, ReadableExt, VNode, use_resource, use_signal};
    use serde_json::Value;

    use super::{
        CatalogLoad, CatalogLookupLoad, DetailLoad, SettlementRead, SettlementReadQuery,
        SettlementReads,
    };
    use crate::api::{
        AdminRecoveryReport, AdminSettlementCatalogEntry, AdminSettlementCatalogPage,
    };
    use crate::surface::auctions::draft_page_tests::{
        RecordRead, button, mount_writer, settle_writer,
    };
    use crate::surface::realtime::text_attributes;
    use crate::surface::test_events::dispatch_click_event;

    const AUCTION: &str = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";

    fn entry() -> AdminSettlementCatalogEntry {
        AdminSettlementCatalogEntry {
            auction_id: AUCTION.to_owned(),
            world_id: "main".to_owned(),
            sequence: 5,
            settled_at: "2025-11-16T09:46:40.110000Z".to_owned(),
        }
    }

    #[test]
    fn each_failed_read_restarts_alone_once_and_only_for_its_original_scope() {
        for (target, read) in [
            SettlementRead::Catalog,
            SettlementRead::CatalogLookup,
            SettlementRead::Settlement,
            SettlementRead::Recovery,
        ]
        .into_iter()
        .enumerate()
        {
            let reply = Rc::new(RecordRead::<()>::new());
            let starts = Rc::new([Cell::new(0), Cell::new(0), Cell::new(0), Cell::new(0)]);
            let trigger = Rc::new(Cell::<Option<Callback<String>>>::new(None));
            let (mut page, _) = mount_writer({
                let reply = reply.clone();
                let starts = starts.clone();
                let trigger = trigger.clone();
                move || {
                    let query = SettlementReadQuery {
                        world: use_signal(|| "main".to_owned()),
                        page: use_signal(|| 1),
                        page_size: use_signal(|| 10),
                        auction_id: use_signal(|| AUCTION.to_owned()),
                        sequence: use_signal(|| Some(5)),
                    };
                    let reload = use_signal(|| 0);
                    let recovery_reload = use_signal(|| 0);
                    let lookup_sequence = dioxus::prelude::use_memo(move || {
                        (read == SettlementRead::CatalogLookup).then_some(5)
                    });
                    let catalog = use_resource({
                        let reply = reply.clone();
                        let starts = starts.clone();
                        move || {
                            starts[0].set(starts[0].get() + 1);
                            let reply = reply.clone();
                            async move {
                                let result = if read == SettlementRead::Catalog {
                                    reply.fetch().await
                                } else {
                                    Ok(())
                                };
                                CatalogLoad {
                                    world_id: "main".to_owned(),
                                    page: 1,
                                    page_size: 10,
                                    result: result.map(|()| AdminSettlementCatalogPage {
                                        auctions: if read == SettlementRead::CatalogLookup {
                                            vec![]
                                        } else {
                                            vec![entry()]
                                        },
                                        page: 1,
                                        page_size: 10,
                                        total: 1,
                                    }),
                                }
                            }
                        }
                    });
                    let lookup = use_resource({
                        let reply = reply.clone();
                        let starts = starts.clone();
                        move || {
                            starts[1].set(starts[1].get() + 1);
                            let reply = reply.clone();
                            async move {
                                let result = if read == SettlementRead::CatalogLookup {
                                    reply.fetch().await
                                } else {
                                    Ok(())
                                };
                                CatalogLookupLoad {
                                    world_id: "main".to_owned(),
                                    sequence: Some(5),
                                    result: result.map(|()| {
                                        Some(AdminSettlementCatalogPage {
                                            auctions: vec![entry()],
                                            page: 1,
                                            page_size: 10,
                                            total: 1,
                                        })
                                    }),
                                }
                            }
                        }
                    });
                    let settlement = use_resource({
                        let reply = reply.clone();
                        let starts = starts.clone();
                        move || {
                            starts[2].set(starts[2].get() + 1);
                            let reply = reply.clone();
                            async move {
                                let result = if read == SettlementRead::Settlement {
                                    reply.fetch().await
                                } else {
                                    Ok(())
                                };
                                let mut body: Value =
                                    serde_json::from_str(include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../auction-contracts/tests/fixtures/admin/settlement-report.json")))
                                        .unwrap();
                                if result.is_err() {
                                    // 传输成功，但归属的世界与目录不符，仍须允许报告原地重试。
                                    body["data"]["auction"]["world_id"] =
                                        Value::String("isekai".to_owned());
                                }
                                DetailLoad {
                                    auction_id: AUCTION.to_owned(),
                                    result: Ok(Some(
                                        serde_json::from_value(body["data"].take()).unwrap(),
                                    )),
                                }
                            }
                        }
                    });
                    let recovery = use_resource({
                        let reply = reply.clone();
                        let starts = starts.clone();
                        move || {
                            starts[3].set(starts[3].get() + 1);
                            let reply = reply.clone();
                            async move {
                                let result = if read == SettlementRead::Recovery {
                                    reply.fetch().await
                                } else {
                                    Ok(())
                                };
                                (
                                    (0, 0),
                                    DetailLoad {
                                        auction_id: AUCTION.to_owned(),
                                        result: result.map(|()| {
                                            Some(AdminRecoveryReport {
                                                auction_id: AUCTION.to_owned(),
                                                recoveries: vec![],
                                            })
                                        }),
                                    },
                                )
                            }
                        }
                    });
                    let reads = SettlementReads {
                        catalog,
                        lookup,
                        settlement,
                        recovery,
                        query,
                        lookup_sequence,
                        reload,
                        recovery_reload,
                    };
                    trigger
                        .set(Some(Callback::new(move |scope: String| reads.retry(read, &scope))));
                    let error = match read {
                        SettlementRead::Catalog => {
                            catalog.read().as_ref().is_some_and(|load| load.result.is_err())
                        }
                        SettlementRead::CatalogLookup => {
                            lookup.read().as_ref().is_some_and(|load| load.result.is_err())
                        }
                        SettlementRead::Settlement => {
                            settlement.read().as_ref().is_some_and(|load| {
                                load.result
                                    .as_ref()
                                    .ok()
                                    .and_then(Option::as_ref)
                                    .is_some_and(|report| report.auction.world_id == "isekai")
                            })
                        }
                        SettlementRead::Recovery => {
                            recovery.read().as_ref().is_some_and(|(_, load)| load.result.is_err())
                        }
                    };
                    if error { reads.error(read, "storage_unavailable") } else { VNode::empty() }
                }
            });
            let scope = match read {
                SettlementRead::Catalog => "main:1:10".to_owned(),
                SettlementRead::CatalogLookup => "main:5".to_owned(),
                SettlementRead::Settlement => AUCTION.to_owned(),
                SettlementRead::Recovery => format!("{AUCTION}:0:0"),
            };
            let retry = trigger.get().unwrap();
            assert_eq!(reply.started(), 1);
            assert_eq!(starts.each_ref().map(Cell::get), [1; 4]);
            reply.respond_unavailable();
            settle_writer(&mut page);
            retry.call("过期的请求身份".to_owned());
            settle_writer(&mut page);
            assert_eq!(reply.started(), 1);
            let (id, _) = button(&page, "重试").expect("读取错误应渲染重试按钮");
            dispatch_click_event(page.dom(), id);
            dispatch_click_event(page.dom(), id);
            let edits = settle_writer(&mut page);
            assert_eq!(button(&page, "重试").map(|(live_id, _)| live_id), Some(id));
            assert!(text_attributes(&edits, "aria-disabled").any(|attr| attr == (id, "true")));
            dispatch_click_event(page.dom(), id);
            settle_writer(&mut page);
            let mut expected = [1; 4];
            expected[target] = 2;
            assert_eq!(starts.each_ref().map(Cell::get), expected);
            assert_eq!(reply.started(), 2);
            reply.respond_unavailable();
            settle_writer(&mut page);
            retry.call(scope.clone());
            settle_writer(&mut page);
            assert_eq!(reply.started(), 3);
            reply.respond(Ok(()));
            settle_writer(&mut page);
            retry.call(scope);
            settle_writer(&mut page);
            assert_eq!(reply.started(), 3);
        }
    }
}
