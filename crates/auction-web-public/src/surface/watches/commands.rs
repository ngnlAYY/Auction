//! 标记命令属于页面的稳定用户宿主；目录 GET、阶段切换和分页不拥有提交任务。

use super::*;
use crate::surface::PublicCommandOwner;

#[derive(Clone)]
struct WatchCommandOwner {
    user_id: Option<String>,
    identity: Signal<Option<String>>,
    session: PublicSessionResource,
    refresh: Signal<u64>,
    commands: Signal<BTreeMap<String, WatchCommand>>,
}

/// 由已按用户 keyed 的页面宿主无条件调用；集合只有一个 Signal，不为每次命令分配 Signal。
pub(crate) fn use_watch_command_owner(user_id: Option<String>) -> Element {
    let mut owner = WatchCommandOwner {
        user_id,
        identity: use_context::<PublicCommandOwner>().0,
        session: use_context::<PublicSessionResource>(),
        refresh: use_context::<Signal<u64>>(),
        commands: use_signal(BTreeMap::new),
    };
    let commands = owner.commands;
    use_context_provider(|| commands);
    let submit = use_context_provider(|| {
        // EventHandler 回到稳定页面 scope；子行与整场读取卸载不取消已提交请求。
        WatchSubmit(EventHandler::new(move |(auction_id, lot_id, payload)| {
            owner.submit(auction_id, lot_id, payload);
        }))
    });
    let snapshot = commands.read();
    let pending = snapshot
        .iter()
        .filter(|(_, command)| command.busy || command.uncertain)
        .map(|(lot_id, command)| (lot_id.clone(), command.clone()));
    rsx! {
        for (lot_id, command) in pending {
            div { key: "{lot_id}", class: "public-lot-watches__sync", role: "status",
                p { "{lot_id}" }
                if command.busy {
                    p { {t!("watch-saving")} }
                } else {
                    p { {t!("watch-unknown")} }
                    button {
                        r#type: "button",
                        onclick: move |_| submit.0.call((
                            command.auction_id.clone(), lot_id.clone(), command.payload.clone(),
                        )),
                        {t!("watch-retry-command")}
                    }
                }
            }
        }
    }
}

impl WatchCommandOwner {
    fn identity_matches(&self) -> bool {
        self.user_id.is_some() && *self.identity.read() == self.user_id
    }

    /// 重试只使用稳定宿主冻结的完整负载；新命令还要求当前会话读取成功。
    fn submit(&mut self, auction_id: String, lot_id: String, payload: PublicSetLotWatchRequest) {
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
                        || pending.auction_id != auction_id
                        || pending.payload.command_id != payload.command_id
                    {
                        return;
                    }
                    // 重试只使用宿主冻结的完整负载，不接受调用方替换版本或新意图。
                    (pending.payload.clone(), pending.uncertain)
                }
                None => {
                    if *self.session.state().read() != UseResourceState::Ready
                        || current_user(&self.session.read()) != Some(user_id.as_str())
                    {
                        return;
                    }
                    (payload, false)
                }
            }
        };
        self.commands.write().insert(
            lot_id.clone(),
            WatchCommand {
                auction_id: auction_id.clone(),
                payload: payload.clone(),
                busy: true,
                uncertain,
                error: None,
                awaiting_refresh: None,
            },
        );
        let mut context = self.clone();
        spawn(async move {
            let result = set_lot_watch(&user_id, &auction_id, &lot_id, &payload).await;
            if !context.identity_matches() {
                return;
            }
            match result {
                Ok(_) => {
                    // 命令重放可能返回旧版本；始终重读集合，绝不直接覆盖权威状态。
                    // 重读完成前保留条目锁住该 lot，界面先显示本次提交的目标状态。
                    let next = context.refresh.peek().saturating_add(1);
                    context.commands.write().insert(
                        lot_id,
                        WatchCommand {
                            auction_id,
                            payload,
                            busy: false,
                            uncertain: false,
                            error: None,
                            awaiting_refresh: Some(next),
                        },
                    );
                    context.refresh.set(next);
                }
                Err(error) => {
                    // 只有路由的明确冲突码能解除未知；裸 409 或损坏响应仍须原样重试。
                    let uncertain = !(error.status == 409 && error.code == "watch_conflict")
                        && (uncertain || error.outcome_uncertain());
                    context.commands.write().insert(
                        lot_id,
                        WatchCommand {
                            auction_id,
                            payload,
                            busy: false,
                            uncertain,
                            error: Some(error),
                            awaiting_refresh: None,
                        },
                    );
                }
            }
        });
    }
}
