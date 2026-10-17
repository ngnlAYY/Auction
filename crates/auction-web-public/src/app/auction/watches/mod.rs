//! 装备标记的整场读取与身份隔离 owner；所有行共享集合，不逐行请求。

mod cache;
mod control;

pub(crate) use control::LotWatchButton;

use super::{PublicSessionResource, login_href_for_current_location};
use crate::api::{CommandError, fetch_lot_watches, set_lot_watch};
use auction_contracts::{PublicLotWatchesResponse, PublicSetLotWatchRequest};
use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use std::{collections::BTreeMap, rc::Rc};

#[derive(Clone)]
struct WatchCommand {
    payload: PublicSetLotWatchRequest,
    busy: bool,
    // 同一命令的未知结果累计保留；后续前置拒绝不能证明早先请求未提交。
    uncertain: bool,
    error: Option<CommandError>,
}

#[derive(Clone, Copy)]
struct WatchSubmit(EventHandler<(String, PublicSetLotWatchRequest)>);

struct LoadedWatches {
    revision: (u64, u64),
    result: Result<Option<PublicLotWatchesResponse>, CommandError>,
}

#[derive(Clone)]
struct WatchContext {
    user_id: Option<String>,
    auction_id: String,
    session: PublicSessionResource,
    refresh: Signal<u64>,
    revision: Signal<u64>,
    resource: Resource<LoadedWatches>,
    cached: Signal<Option<Rc<PublicLotWatchesResponse>>>,
    commands: Signal<BTreeMap<String, WatchCommand>>,
}

/// 身份变化会销毁整个命令宿主；匿名从不读写账号缓存或提交标记。
#[component]
pub(crate) fn LotWatchScope(auction_id: String, children: Element) -> Element {
    let session = use_context::<PublicSessionResource>();
    let user_id = current_user(session);
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
        user_id.as_deref().and_then(|user| cache::load(user, &auction_id)).map(Rc::new)
    });
    let commands = use_signal(BTreeMap::new);
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
                    if let Ok(page) = &result {
                        cache::save(page);
                        cached.set(Some(Rc::new(page.clone())));
                    }
                    result.map(Some)
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
    use_context_provider(|| {
        let mut owner = context.clone();
        // EventHandler 回到创建它的整场 scope，分页卸载单行不会取消提交并遗留 busy。
        WatchSubmit(EventHandler::new(move |(lot_id, payload)| owner.submit(lot_id, payload)))
    });
    let authenticated = context.identity_matches();
    let ready = context.ready();
    let failed = resource.read().as_ref().is_some_and(|loaded| loaded.result.is_err());
    let mut retry = context.clone();

    rsx! {
        div { class: "public-lot-watches",
            p { class: "public-lot-watches__help",
                {t!("watch-help")}
                " "
                Link { to: "/me#outbid-notifications", {t!("watch-preference-link")} }
            }
            if authenticated && !ready {
                div {
                    class: "public-lot-watches__sync",
                    role: if failed { "alert" } else { "status" },
                    p {
                        if failed { {t!("watch-load-error")} } else { {t!("watch-loading")} }
                    }
                    if cached.read().is_some() {
                        p { {t!("watch-unsynced")} }
                    }
                    if failed {
                        button {
                            r#type: "button",
                            onclick: move |_| retry.invalidate(),
                            {t!("watch-refresh")}
                        }
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
        Some(
            page.watches
                .iter()
                .find(|watch| watch.lot_id == lot_id)
                .map_or((false, 0), |watch| (watch.watched, watch.version)),
        )
    }

    fn invalidate(&mut self) {
        let next = self.revision.peek().saturating_add(1);
        self.revision.set(next);
    }

    /// 提交前后都检查当前会话；拍卖由 keyed owner 固定，卸载取消所属任务。
    fn submit(&mut self, lot_id: String, payload: PublicSetLotWatchRequest) {
        if !self.identity_matches() {
            return;
        }
        let Some(user_id) = self.user_id.clone() else { return };
        let (payload, uncertain) = {
            let commands = self.commands.peek();
            match commands.get(&lot_id) {
                Some(pending) => {
                    if pending.busy
                        || !pending.uncertain
                        || pending.payload.command_id != payload.command_id
                    {
                        return;
                    }
                    // 重试只使用宿主冻结的完整负载，不接受调用方替换版本或新意图。
                    (pending.payload.clone(), pending.uncertain)
                }
                None => (payload, false),
            }
        };
        self.commands.write().insert(
            lot_id.clone(),
            WatchCommand { payload: payload.clone(), busy: true, uncertain, error: None },
        );
        let mut context = self.clone();
        spawn(async move {
            if !context.identity_matches() {
                return;
            }
            let result = set_lot_watch(&user_id, &context.auction_id, &lot_id, &payload).await;
            if !context.identity_matches() {
                return;
            }
            match result {
                Ok(_) => {
                    context.commands.write().remove(&lot_id);
                    // 命令重放可能返回旧版本；始终重读集合，绝不直接覆盖权威状态。
                    context.invalidate();
                }
                Err(error) => {
                    // 只有成功或明确 409 可解除未知状态；429 等拒绝只描述本次重试。
                    let uncertain = error.status != 409 && (uncertain || error.outcome_uncertain());
                    context.commands.write().insert(
                        lot_id,
                        WatchCommand { payload, busy: false, uncertain, error: Some(error) },
                    );
                }
            }
        });
    }
}
