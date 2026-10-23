//! 装备标记的整场读取与身份隔离 owner；所有行共享集合，不逐行请求。

mod cache;
mod commands;
mod control;

pub(crate) use commands::use_watch_command_owner;
pub(crate) use control::LotWatchButton;

use super::{PublicSessionResource, login_href_for_current_location};
use crate::api::{CommandError, fetch_lot_watches, set_lot_watch};
use auction_contracts::{PublicLotWatchesResponse, PublicSetLotWatchRequest};
use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use std::{collections::BTreeMap, rc::Rc};

#[derive(Clone)]
struct WatchCommand {
    auction_id: String,
    payload: PublicSetLotWatchRequest,
    busy: bool,
    // 同一命令的未知结果累计保留；后续前置拒绝不能证明早先请求未提交。
    uncertain: bool,
    error: Option<CommandError>,
}

#[derive(Clone, Copy)]
struct WatchSubmit(EventHandler<(String, String, PublicSetLotWatchRequest)>);

/// 完整集合只索引一次，所有目录行按 lot UUID 查状态；false 墓碑保留其真实版本。
type WatchSnapshot = BTreeMap<String, (bool, i64)>;

fn watch_snapshot(page: PublicLotWatchesResponse) -> Rc<WatchSnapshot> {
    Rc::new(
        page.watches
            .into_iter()
            .map(|watch| (watch.lot_id, (watch.watched, watch.version)))
            .collect(),
    )
}

struct LoadedWatches {
    revision: (u64, u64),
    result: Result<Option<Rc<WatchSnapshot>>, CommandError>,
}

#[derive(Clone)]
struct WatchContext {
    user_id: Option<String>,
    auction_id: String,
    session: PublicSessionResource,
    refresh: Signal<u64>,
    revision: Signal<u64>,
    resource: Resource<LoadedWatches>,
    cached: Signal<Option<Rc<WatchSnapshot>>>,
    commands: Signal<BTreeMap<String, WatchCommand>>,
}

/// 整场读取按稳定用户隔离；提交集合和任务由外层页面宿主持有。
#[component]
pub(crate) fn LotWatchScope(auction_id: String, children: Element) -> Element {
    let user_id = use_context::<super::PublicCommandOwner>().0();
    let scope = format!("{}:{}", user_id.as_deref().unwrap_or_default(), auction_id);
    rsx! {
        ScopedLotWatches { key: "{scope}", user_id, auction_id, children }
    }
}

#[component]
fn ScopedLotWatches(user_id: Option<String>, auction_id: String, children: Element) -> Element {
    let session = use_context::<PublicSessionResource>();
    let refresh = use_context::<Signal<u64>>();
    let revision = use_signal(|| 0_u64);
    let mut cached = use_signal(|| {
        user_id.as_deref().and_then(|user| cache::load(user, &auction_id)).map(watch_snapshot)
    });
    let commands = use_context::<Signal<BTreeMap<String, WatchCommand>>>();
    let resource = use_resource({
        let user_id = user_id.clone();
        let auction_id = auction_id.clone();
        move || {
            let requested_revision = (refresh(), revision());
            let user_id = user_id.clone();
            let auction_id = auction_id.clone();
            async move {
                let result = if let Some(user_id) = user_id {
                    if current_user(session).as_deref() != Some(&user_id) {
                        return LoadedWatches { revision: requested_revision, result: Ok(None) };
                    }
                    let result = fetch_lot_watches(&user_id, &auction_id).await;
                    if current_user(session).as_deref() != Some(&user_id) {
                        return LoadedWatches { revision: requested_revision, result: Ok(None) };
                    }
                    result.map(|page| {
                        cache::save(&page);
                        let snapshot = watch_snapshot(page);
                        cached.set(Some(Rc::clone(&snapshot)));
                        Some(snapshot)
                    })
                } else {
                    Ok(None)
                };
                LoadedWatches { revision: requested_revision, result }
            }
        }
    });
    let context = WatchContext {
        user_id,
        auction_id,
        session,
        refresh,
        revision,
        resource,
        cached,
        commands,
    };
    use_context_provider(|| context.clone());
    let authenticated = context.identity_matches();
    let ready = context.ready();
    let failed = resource.read().as_ref().is_some_and(|loaded| loaded.result.is_err());
    let mut retry = context.clone();

    rsx! {
        div { class: "public-lot-watches", aria_busy: (authenticated && !ready && !failed).to_string(),
            if authenticated && !ready && failed {
                div {
                    class: "public-lot-watches__sync",
                    role: "alert",
                    p { {t!("watch-load-error")} }
                    button {
                        r#type: "button",
                        onclick: move |_| retry.invalidate(),
                        {t!("watch-refresh")}
                    }
                }
            }
            {children}
        }
    }
}

fn current_user(session: PublicSessionResource) -> Option<String> {
    session.read().as_ref().and_then(|snapshot| {
        snapshot
            .as_ref()
            .ok()
            .and_then(|identity| identity.as_ref())
            .filter(|identity| identity.authenticated)
            .map(|identity| identity.user_id.clone())
    })
}

impl WatchContext {
    fn identity_matches(&self) -> bool {
        self.user_id.is_some() && current_user(self.session) == self.user_id
    }

    fn ready(&self) -> bool {
        self.identity_matches()
            && *self.session.state().read() == UseResourceState::Ready
            && *self.resource.state().read() == UseResourceState::Ready
            && self.resource.read().as_ref().is_some_and(|loaded| {
                loaded.revision == ((self.refresh)(), (self.revision)())
                    && matches!(&loaded.result, Ok(Some(_)))
            })
    }

    /// 只有完整且当前的服务器集合才产生 CAS 版本；缓存绝不进入此分支。
    fn authoritative(&self, lot_id: &str) -> Option<(bool, i64)> {
        if !self.ready() {
            return None;
        }
        let snapshot = self.resource.read();
        let page = snapshot.as_ref()?.result.as_ref().ok()?.as_ref()?;
        Some(page.get(lot_id).copied().unwrap_or((false, 0)))
    }

    fn invalidate(&mut self) {
        let next = self.revision.peek().saturating_add(1);
        self.revision.set(next);
    }
}
