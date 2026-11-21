//! 装备标记的整场读取与身份隔离 owner；所有行共享集合，不逐行请求。

pub(super) mod cache;
pub(super) mod commands;
pub(super) mod control;

#[cfg(test)]
pub(super) use cache::{load as load_watch_cache, save as save_watch_cache};
pub(crate) use commands::use_watch_command_owner;
pub(crate) use control::{LotWatchButton, use_lot_watched};

use std::{collections::BTreeMap, rc::Rc};

use auction_contracts::{PublicLotWatchesResponse, PublicSetLotWatchRequest};
use auction_web_shared::t;
use dioxus::prelude::*;

use super::commands::session::{CommandSession, SessionAuthority};
use super::{PublicCommandOwner, PublicSessionResource};
use crate::api::{CommandError, PublicSession, fetch_lot_watches};

#[derive(Clone)]
struct WatchCommand {
    auction_id: String,
    /// 拍品编号只用于页面级状态条辨认装备，留在宿主内存，不进入请求。
    lot_code: String,
    payload: PublicSetLotWatchRequest,
    busy: bool,
    // 同一命令的未知结果累计保留；后续前置拒绝不能证明早先请求未提交。
    uncertain: bool,
    error: Option<CommandError>,
    /// 成功后等待的 [`WatchRevision`]；读到包含本次结果的集合前锁住该 lot，不用旧版本发起 CAS。
    awaiting_refresh: Option<u64>,
}

impl WatchCommand {
    fn not_sent(&self) -> bool {
        self.error.as_ref().is_some_and(CommandError::not_sent)
    }
}

/// 提交负载：场次、拍品 UUID、拍品编号与冻结的命令 DTO。
#[derive(Clone, Copy)]
struct WatchSubmit(EventHandler<(String, String, String, PublicSetLotWatchRequest)>);

/// 标记命令成功后的整场集合重读修订号，由命令宿主持有。标记状态只在 `/me/lot-watches`
/// 集合里，成功只需重读该集合，不推进全局刷新去重读当前场次与目录。用独立类型提供上下文，
/// 不遮蔽外壳的全局刷新 `Signal<u64>`。
#[derive(Clone, Copy)]
struct WatchRevision(Signal<u64>);

/// 账号边界销毁旧快照；同账号 Cookie 对账经独立修订重读标记。
#[derive(Clone, Copy)]
pub(super) struct WatchSessionRevision {
    pub boundary: Signal<u64>,
    pub refresh: Signal<u64>,
}

/// 完整集合只索引一次，所有目录行按 lot UUID 查状态；false 墓碑保留其真实版本。
type WatchSnapshot = BTreeMap<String, (bool, i64)>;

/// 看板计数测试保留真实标记作用域，隔离浏览器传输。
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct SkipWatchReads;

fn watch_snapshot(page: &PublicLotWatchesResponse) -> Rc<WatchSnapshot> {
    Rc::new(
        page.watches
            .iter()
            .map(|watch| (watch.lot_id.clone(), (watch.watched, watch.version)))
            .collect(),
    )
}

/// 一次整场读取的结果及其发起时的（全局刷新、标记修订、本地失效）修订号。
struct LoadedWatches {
    revision: (u64, u64, u64),
    primary_revision: Option<String>,
    result: Result<Option<WatchRead>, CommandError>,
}

struct WatchRead {
    page: PublicLotWatchesResponse,
    snapshot: Rc<WatchSnapshot>,
    cache_revision: Option<String>,
}

#[derive(Clone)]
struct WatchContext {
    user_id: Option<String>,
    auction_id: String,
    session: PublicSessionResource,
    revision: Signal<u64>,
    resource: Resource<LoadedWatches>,
    authority: SessionAuthority,
    accepted: Signal<Option<Rc<WatchSnapshot>>>,
    cached: Signal<Option<Rc<WatchSnapshot>>>,
    commands: Signal<BTreeMap<String, WatchCommand>>,
    owner: Signal<Option<String>>,
    boundary: Option<(Signal<u64>, u64)>,
}

/// 整场读取按稳定用户隔离；提交集合和任务由外层页面宿主持有。
#[component]
pub(crate) fn LotWatchScope(auction_id: String, children: Element) -> Element {
    let user_id = use_context::<PublicCommandOwner>().0();
    let boundary =
        try_use_context::<WatchSessionRevision>().map_or(0, |revision| (revision.boundary)());
    let scope = format!("{boundary}:{}:{auction_id}", user_id.as_deref().unwrap_or_default());
    rsx! {
        for (scope, user_id, auction_id, children) in [(scope, user_id, auction_id, children)] {
            ScopedLotWatches { key: "{scope}", user_id, auction_id, children }
        }
    }
}

#[component]
fn ScopedLotWatches(user_id: Option<String>, auction_id: String, children: Element) -> Element {
    let session = use_context::<PublicSessionResource>();
    let owner = use_context::<PublicCommandOwner>().0;
    let session_revision = try_use_context::<WatchSessionRevision>();
    let boundary = use_hook(|| {
        session_revision.map(|revision| (revision.boundary, *revision.boundary.peek()))
    });
    let refresh = use_context::<Signal<u64>>();
    let watch_revision = use_context::<WatchRevision>().0;
    let revision = use_signal(|| 0_u64);
    let mut cached = use_signal(|| {
        user_id
            .as_deref()
            .and_then(|user| cache::load(user, &auction_id))
            .as_ref()
            .map(watch_snapshot)
    });
    let authority = SessionAuthority::use_context();
    let mut accepted = use_signal(|| None);
    let mut commands = use_context::<Signal<BTreeMap<String, WatchCommand>>>();
    #[cfg(test)]
    let skip_watch_reads = try_use_context::<SkipWatchReads>().is_some();
    // 只跟随可读身份的变化：会话故障恢复后立即补读，同账号续期不重启整场请求。
    let readable = use_memo({
        let user_id = user_id.clone();
        move || {
            #[cfg(test)]
            if skip_watch_reads {
                return false;
            }
            user_id.as_deref().is_some_and(|user_id| current_user(&session.read()) == Some(user_id))
        }
    });
    let resource = use_resource({
        let user_id = user_id.clone();
        let auction_id = auction_id.clone();
        move || {
            let requested_revision = (refresh(), watch_revision(), revision());
            if let Some(session_revision) = session_revision {
                (session_revision.refresh)();
            }
            let readable = readable();
            let confirmation = authority.confirmed();
            let user_id = user_id.clone();
            let auction_id = auction_id.clone();
            async move {
                let mut loaded = LoadedWatches {
                    revision: requested_revision,
                    primary_revision: confirmation
                        .as_ref()
                        .and_then(|confirmed| confirmed.primary_revision.clone()),
                    result: Ok(None),
                };
                let Some(user_id) = user_id.filter(|_| readable && confirmation.is_some()) else {
                    return loaded;
                };
                let current = || {
                    matches_watch_owner(owner, boundary, &user_id)
                        && current_user(&session.peek()) == Some(user_id.as_str())
                        && authority.current() == confirmation
                };
                if !current() {
                    return loaded;
                }
                let result = fetch_lot_watches(&user_id, &auction_id).await;
                if !current() {
                    return loaded;
                }
                loaded.result = match result {
                    Err(error) if error.code == "session_observation_stale" => Ok(None),
                    result => result.map(|(page, observation)| {
                        if !observation.current()
                            || observation.primary_revision() != loaded.primary_revision.as_deref()
                        {
                            return None;
                        }
                        let snapshot = watch_snapshot(&page);
                        Some(WatchRead {
                            page,
                            snapshot,
                            cache_revision: observation.cache_revision(),
                        })
                    }),
                };
                loaded
            }
        }
    });
    let context = WatchContext {
        user_id,
        auction_id,
        session,
        revision,
        resource,
        authority,
        accepted,
        cached,
        commands,
        owner,
        boundary,
    };
    let delivery = context.clone();
    use_effect(move || {
        let current = resource.read();
        let Some(loaded) = current.as_ref() else { return };
        let Ok(Some(read)) = &loaded.result else { return };
        if !delivery.accepts(loaded)
            || accepted.peek().as_ref().is_some_and(|snapshot| Rc::ptr_eq(snapshot, &read.snapshot))
        {
            return;
        }
        // 交付才可更新缓存；另一页在检查与写入之间换代时，持久读回仍拒绝旧修订。
        cache::save(&read.page, read.cache_revision.as_deref());
        if !delivery.accepts(loaded) {
            return;
        }
        cached.set(Some(Rc::clone(&read.snapshot)));
        accepted.set(Some(Rc::clone(&read.snapshot)));
        let settled = settled_lots(&commands.peek(), &read.page.auction_id, loaded.revision.1);
        if !settled.is_empty() {
            let mut commands = commands.write();
            for lot_id in settled {
                commands.remove(&lot_id);
            }
        }
    });
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

/// 本场可以解锁的成功命令：等待的标记修订号不大于已读集合发起时的修订号，说明该集合是在
/// 命令成功之后读取的，已包含本次结果。忙碌、结果未知或失败的条目没有等待值，保持原样。
fn settled_lots(
    commands: &BTreeMap<String, WatchCommand>,
    auction_id: &str,
    loaded_watch_revision: u64,
) -> Vec<String> {
    commands
        .iter()
        .filter(|(_, command)| {
            command.auction_id == auction_id
                && command.awaiting_refresh.is_some_and(|target| target <= loaded_watch_revision)
        })
        .map(|(lot_id, _)| lot_id.clone())
        .collect()
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

fn matches_watch_owner(
    owner: Signal<Option<String>>,
    boundary: Option<(Signal<u64>, u64)>,
    user_id: &str,
) -> bool {
    owner.read().as_deref() == Some(user_id)
        && boundary.is_none_or(|(revision, mounted)| revision() == mounted)
}

impl WatchContext {
    fn identity_matches(&self) -> bool {
        self.user_id.as_deref().is_some_and(|user_id| {
            matches_watch_owner(self.owner, self.boundary, user_id)
                && current_user(&self.session.read()) == Some(user_id)
        })
    }

    fn accepts(&self, loaded: &LoadedWatches) -> bool {
        self.identity_matches()
            && self
                .authority
                .current()
                .is_some_and(|confirmed| confirmed.primary_revision == loaded.primary_revision)
            && loaded.revision.2 == (self.revision)()
    }

    fn ready(&self) -> bool {
        self.authoritative_snapshot().is_some()
    }

    fn authoritative_snapshot(&self) -> Option<Rc<WatchSnapshot>> {
        let resource = self.resource.read();
        let loaded = resource.as_ref().filter(|loaded| self.accepts(loaded))?;
        let read = loaded.result.as_ref().ok()?.as_ref()?;
        self.accepted
            .read()
            .as_ref()
            .filter(|accepted| Rc::ptr_eq(accepted, &read.snapshot))
            .cloned()
    }

    /// 只有当前代次已交付的服务器集合才提供 CAS 版本。
    fn toggle_baseline(&self, session: &CommandSession, lot_id: &str) -> Option<(bool, i64)> {
        if !session.can_submit_new()
            || self.commands.read().get(lot_id).is_some_and(|command| !command.not_sent())
        {
            return None;
        }
        let snapshot = self.authoritative_snapshot()?;
        Some(snapshot.get(lot_id).copied().unwrap_or((false, 0)))
    }

    /// 旧集合可继续提供展示；刚提交成功的目标状态优先显示到重读收口。
    fn displayed(&self, lot_id: &str) -> bool {
        if !self.identity_matches() {
            return false;
        }
        let settled = self
            .commands
            .read()
            .get(lot_id)
            .filter(|pending| pending.awaiting_refresh.is_some())
            .map(|pending| pending.payload.watched);
        settled.unwrap_or_else(|| {
            self.cached
                .read()
                .as_ref()
                .is_some_and(|page| page.get(lot_id).is_some_and(|(watched, _)| *watched))
        })
    }

    fn invalidate(&mut self) {
        let next = self.revision.peek().saturating_add(1);
        self.revision.set(next);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use auction_contracts::PublicSetLotWatchRequest;

    use super::{WatchCommand, settled_lots};
    use crate::api::CommandError;

    fn command(auction_id: &str, awaiting_refresh: Option<u64>) -> WatchCommand {
        WatchCommand {
            auction_id: auction_id.to_owned(),
            lot_code: "Clo16".to_owned(),
            payload: PublicSetLotWatchRequest {
                command_id: "command-1".to_owned(),
                watched: true,
                channel: "mm".to_owned(),
                expected_version: 3,
            },
            busy: false,
            uncertain: false,
            error: None,
            awaiting_refresh,
        }
    }

    #[test]
    fn success_unlocks_only_after_a_collection_read_started_after_it() {
        let commands = BTreeMap::from([("lot-1".to_owned(), command("auction-7", Some(4)))]);
        // 命令成功前发起的读取不含本次结果，lot 保持锁定。
        assert!(settled_lots(&commands, "auction-7", 3).is_empty());
        assert_eq!(settled_lots(&commands, "auction-7", 4), vec!["lot-1".to_owned()]);
        assert_eq!(settled_lots(&commands, "auction-7", 5), vec!["lot-1".to_owned()]);
    }

    #[test]
    fn pending_failed_and_other_auction_commands_stay_locked() {
        let mut busy = command("auction-7", None);
        busy.busy = true;
        let mut uncertain = command("auction-7", None);
        uncertain.uncertain = true;
        uncertain.error = Some(CommandError { code: "network_error".to_owned(), status: 0 });
        let commands = BTreeMap::from([
            ("lot-busy".to_owned(), busy),
            ("lot-uncertain".to_owned(), uncertain),
            ("lot-other".to_owned(), command("auction-8", Some(1))),
        ]);
        assert!(settled_lots(&commands, "auction-7", 9).is_empty());
    }
}
