//! 装备标记的整场读取与身份隔离 owner；所有行共享集合，不逐行请求。

mod cache;
mod commands;
mod control;

pub(crate) use commands::use_watch_command_owner;
pub(crate) use control::{LotWatchButton, use_lot_watched};

use std::{collections::BTreeMap, rc::Rc};

use auction_contracts::{PublicLotWatchesResponse, PublicSetLotWatchRequest};
use auction_web_shared::t;
use dioxus::prelude::*;

use super::PublicSessionResource;
use crate::api::{CommandError, PublicSession, fetch_lot_watches};

#[derive(Clone)]
struct WatchCommand {
    auction_id: String,
    payload: PublicSetLotWatchRequest,
    busy: bool,
    // 同一命令的未知结果累计保留；后续前置拒绝不能证明早先请求未提交。
    uncertain: bool,
    error: Option<CommandError>,
    /// 成功后等待的整场重读修订号；读到包含本次结果的集合前锁住该 lot，不用旧版本发起 CAS。
    awaiting_refresh: Option<u64>,
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
        for (scope, user_id, auction_id, children) in [(scope, user_id, auction_id, children)] {
            ScopedLotWatches { key: "{scope}", user_id, auction_id, children }
        }
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
    let mut commands = use_context::<Signal<BTreeMap<String, WatchCommand>>>();
    // 只跟随可读身份的变化：会话故障恢复后立即补读，同账号续期不重启整场请求。
    let readable = use_memo({
        let user_id = user_id.clone();
        move || {
            user_id.as_deref().is_some_and(|user_id| current_user(&session.read()) == Some(user_id))
        }
    });
    let resource = use_resource({
        let user_id = user_id.clone();
        let auction_id = auction_id.clone();
        move || {
            let requested_revision = (refresh(), revision());
            let readable = readable();
            let user_id = user_id.clone();
            let auction_id = auction_id.clone();
            async move {
                if !readable {
                    return LoadedWatches { revision: requested_revision, result: Ok(None) };
                }
                let result = if let Some(user_id) = user_id {
                    // 请求前后复核身份，不能把会话切换期间的响应写入当前缓存。
                    if current_user(&session.peek()) != Some(user_id.as_str()) {
                        return LoadedWatches { revision: requested_revision, result: Ok(None) };
                    }
                    let result = fetch_lot_watches(&user_id, &auction_id).await;
                    if current_user(&session.peek()) != Some(user_id.as_str()) {
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
    // 成功命令要等包含其结果的整场重读成功后才解锁对应 lot；只处理本场条目。
    let settled_auction_id = auction_id.clone();
    use_effect(move || {
        let Some(loaded_refresh) = resource
            .read()
            .as_ref()
            .filter(|loaded| matches!(&loaded.result, Ok(Some(_))))
            .map(|loaded| loaded.revision.0)
        else {
            return;
        };
        let settled: Vec<String> = commands
            .read()
            .iter()
            .filter(|(_, command)| {
                command.auction_id == settled_auction_id
                    && command.awaiting_refresh.is_some_and(|target| target <= loaded_refresh)
            })
            .map(|(lot_id, _)| lot_id.clone())
            .collect();
        if !settled.is_empty() {
            let mut commands = commands.write();
            for lot_id in settled {
                commands.remove(&lot_id);
            }
        }
    });
    let context =
        WatchContext { user_id, auction_id, session, revision, resource, cached, commands };
    use_context_provider(|| context.clone());
    let authenticated = context.identity_matches();
    let ready = context.ready();
    let failed = resource.read().as_ref().is_some_and(|loaded| loaded.result.is_err());
    let mut retry = context;

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

fn current_user(snapshot: &Option<Result<Option<PublicSession>, String>>) -> Option<&str> {
    snapshot.as_ref().and_then(|snapshot| {
        snapshot
            .as_ref()
            .ok()
            .and_then(|identity| identity.as_ref())
            .filter(|identity| identity.authenticated)
            .map(|identity| identity.user_id.as_str())
    })
}

impl WatchContext {
    fn identity_matches(&self) -> bool {
        self.user_id.is_some() && current_user(&self.session.read()) == self.user_id.as_deref()
    }

    /// 本作用域最近一次成功的服务器集合可用；全局定时或竞价触发的后台重读期间沿用它，
    /// 按钮不随请求往返闪烁。本地失效（重试、手动刷新）仍须等重读完成。
    fn ready(&self) -> bool {
        self.identity_matches()
            && self.resource.read().as_ref().is_some_and(|loaded| {
                loaded.revision.1 == (self.revision)() && matches!(&loaded.result, Ok(Some(_)))
            })
    }

    /// 只有服务器集合才产生 CAS 版本，缓存绝不进入此分支；刚提交成功的 lot 由命令条目单独锁住，
    /// 其他 lot 的版本在后台重读期间不变，过期时由服务端 409 失败关闭。
    fn authoritative(&self, lot_id: &str) -> Option<(bool, i64)> {
        if !self.ready() {
            return None;
        }
        let snapshot = self.resource.read();
        let page = snapshot.as_ref()?.result.as_ref().ok()?.as_ref()?;
        Some(page.get(lot_id).copied().unwrap_or((false, 0)))
    }

    /// 展示用的标记状态：成功提交后等待重读期间取提交的目标状态（标签不先回落到旧值再翻转），
    /// 其次是服务器集合，最后才是本地缓存。只决定图标与文案，不产生 CAS 版本。
    fn displayed(&self, lot_id: &str) -> bool {
        let settled = self
            .commands
            .read()
            .get(lot_id)
            .filter(|pending| pending.awaiting_refresh.is_some())
            .map(|pending| pending.payload.watched);
        settled.or_else(|| self.authoritative(lot_id).map(|(watched, _)| watched)).unwrap_or_else(
            || {
                self.cached
                    .read()
                    .as_ref()
                    .is_some_and(|page| page.get(lot_id).is_some_and(|(watched, _)| *watched))
            },
        )
    }

    fn invalidate(&mut self) {
        let next = self.revision.peek().saturating_add(1);
        self.revision.set(next);
    }
}
